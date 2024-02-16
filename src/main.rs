use std::path::PathBuf;
use std::fs::File;
use std::io;
use clap::{Parser, command};
use packet::{Packet,PacketPayload,ShowPacket,EffectId};
use log::{debug,info,warn,error};
use crossbeam_channel::bounded;
use anyhow::{anyhow,Result,Context};
use std::thread;
use signal_hook::consts::{SIGINT,SIGTERM,SIGHUP};
use signal_hook::iterator::SignalsInfo;
use signal_hook::iterator::exfiltrator::WithOrigin;

use crate::radio::Radio;
use crate::director::{Director,DirectorMessage};
use crate::show::Color;

pub mod config;
pub mod radio;
pub mod midi;
pub mod packet;
pub mod show;
pub mod director;
pub mod showstate;
pub mod clip;

const DEFAULT_BUFFER_SIZE: usize = 10;

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

fn main() -> anyhow::Result<()> {
    env_logger::init();

    let cli = Cli::parse();
    debug!("Command line arguments: {:?}", cli);

    let config = load_config(&cli)
        .context("Error parsing configuration")?;
    info!("Loaded configuration: {:?}", config);

    // initialize our midi and radio libraries/interfaces
    info!("Initializing MIDI...");
    let midi_in = midi::midi_init(&config)?;

    info!("Initializing radio...");
    let mut radio = Radio::init(&config)?;

    // handle some command line options that do some work and then terminate early
    match cli {
        Cli { enumerate_midi: true, ..} => {
            midi::midi_enum(&midi_in);
            return Ok(())
        },
        Cli { all_on: true, ..} => {
            all_on(&mut radio);
            return Ok(())
        }
        _ => {}
    }
    
    // create a channel to send midi back to the
    // main thread from the midirs thread
    let (tx, rx) = 
        bounded(config.channel_buf_depth.unwrap_or(DEFAULT_BUFFER_SIZE));

    let midi_tx = tx.clone();

    if let Some(port) = midi::find_port(&midi_in, &config.midi_port) {
        let midi_connection = midi_in.connect(&port, "chs-lights-in", 
                    move | ts, midi_bytes, _ | 
                        { midi_tx.send(DirectorMessage::MidiMessage { ts, buf: midi_bytes.to_owned() }).unwrap(); }, ()).unwrap();
        
        // create a director and give it the receive channel, the config, and the radio
        // note the director takes ownership of the config, radio, and receiver
        let mut director = Director::new(config, radio, rx);

        // launch the show in its own thread
        let join_handle = thread::spawn(move || { 
            if let Err(e) = director.run_show() { 
                error!("Show terminated early with error: {}", e);
            } 
        });

        // listen for signals and forward them to the director
        let sigs = vec![
            // initiate shutdown
            SIGINT, 
            SIGTERM,
            // reload show from JSON
            SIGHUP,
        ];
        
        let mut signals = SignalsInfo::<WithOrigin>::new(&sigs)?;

        if !join_handle.is_finished() {
            for info in &mut signals {
                debug!("In signal handling loop");
                match info.signal {
                    SIGINT | SIGTERM => {
                        tx.send(DirectorMessage::Shutdown)?;
                        break;
                    },
                    SIGHUP => { tx.send(DirectorMessage::Reload)?; },
                    x => { warn!("Unexpected signal: {}", x); }
                }
            }
        }
        debug!("Exited signal handling loop");


        // note the connection must be kept alive until the show is over, 
        // otherwise midirs will close the connection
        drop(midi_connection);

        // join the show thread before shutdown
        let _ = join_handle.join();
        Ok(())
    } else {
        Err(anyhow!("No MIDI port matches prefix: {:?}", config.midi_port))
    }
}

fn all_on(radio: &mut Radio) {
    let all_on = Packet {
        recipients: &vec![],
        payload: PacketPayload::Show(
            ShowPacket {
                effect: EffectId::Pop,
                color: Color { h: 0, s: 0, v: 255 },
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
