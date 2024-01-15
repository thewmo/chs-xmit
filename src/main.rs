use std::path::PathBuf;
use std::fs::File;
use std::io;
use clap::{Parser, command};
use packet::{Packet,PacketPayload,ShowPacket,EffectId};
use std::sync::mpsc;
use std::thread;
use log::{debug,info,error};

use radio::Radio;
use types::Color;

pub mod config;
pub mod types;
pub mod radio;
pub mod midi;
pub mod packet;
pub mod show;
pub mod director;

#[derive(Parser, Debug)]
#[command(author, version)]
#[command(about = "CHS Band Lights Transmitter")]
struct Cli {

    #[arg(short, long, value_name = "FILE")]
    config: PathBuf,

    #[arg(short, long)]
    debug: bool,

    #[arg(short, long)]
    enumerate_midi: bool,

    /// if true, just send an "all on white" packet
    /// and exit, for troubleshooting purposes
    #[arg(short, long)]
    all_on: bool

}

fn load_config(cli: &Cli) -> Result<config::ConfigFile, io::Error> {
    let file = File::open(&cli.config)?;
    Ok(serde_json::from_reader(&file)?)
}

fn main() {
    env_logger::init();

    let cli = Cli::parse();
    debug!("Command line arguments: {:?}", cli);

    let config = load_config(&cli).unwrap();
    debug!("Configuration file: {:?}", config);

    // initialize our midi and radio libraries/interfaces
    let midi_in = midi::midi_init(&config).unwrap();
    let mut radio = Radio::init(&config).unwrap();

    // handle some command line options that do some work and then terminate early
    match cli {
        Cli { enumerate_midi: true, ..} => {
            midi::midi_enum(&midi_in);
            return;
        },
        Cli { all_on: true, ..} => {
            all_on(&mut radio);
            return;
        }
        _ => ()
    }
    
    // create a channel to send midi back to the
    // main thread from the midirs thread
    let (tx, rx) = mpsc::channel();

    if let Some(port) = midi::find_port(&midi_in, &config.midi_port) {
        let midi_connection = midi_in.connect(&port, "chs-lights-in", 
                    move | ts, buf, _ | { tx.send((ts, buf.to_owned())).unwrap(); }, ()).unwrap();
        // now, our state machine can read from the channel, processing midi
        // as it goes
        // create a director and give it the receive channel, the config, and the radio
    } else {
        error!("No MIDI port matching prefix: {}", config.midi_port);
    }
}

fn all_on(radio: &mut Radio) {
    let all_on = Packet {
        recipients: vec![],
        payload: PacketPayload::Show(
            ShowPacket {
                effect: EffectId::Pop,
                color: Color { hue: 0, saturation: 0, brightness: 255 },
                attack: 0,
                sustain: 255,
                release: 0,
                param1: 0,
                param2: 0,
                tempo: 0
            })
    };

    radio.send(&all_on).unwrap();
}
