use std::path::PathBuf;
use clap::{Parser, command};
use log::debug;
use midir::{MidiInput, MidiInputPort, MidiInputPorts};

pub mod radio;
pub mod midi;

#[derive(Parser, Debug)]
#[command(author, version)]
#[command(about = "CHS Band Lights Transmitter")]
struct Cli {

    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    #[arg(short, long)]
    debug: bool,

    #[arg(short, long)]
    enumerate_midi: bool,

    #[arg(short, long, value_name = "PORT_NUM", help = "The MIDI port number to use, as returned from enumerate output.")]
    midi_port: usize
}

fn main() {
    env_logger::init();
    let cli = Cli::parse();

    let midi_in = MidiInput::new("chslights").unwrap();
    let midi_in_ports = midi_in.ports();

    if cli.enumerate_midi {
        midi_enum(&midi_in, &midi_in_ports);
        return;
    }
    let radio = radio::radio_init().unwrap();

    match midi_in_ports.get(cli.midi_port) {
        None => {
            eprintln!("Unknown midi port number specified: {}", cli.midi_port);
            std::process::exit(1);
        }
        Some(p) => process_midi(&midi_in, &p)
    };
}

/// enumerate the midi ports available on the system
fn midi_enum(input: &MidiInput, ports: &MidiInputPorts) {
    println!("Available Midi Ports");
    println!("====================");
    for (i, p) in ports.iter().enumerate() {
        println!("{}: {}", i, input.port_name(p).unwrap());
    }
}

fn process_midi(input: &MidiInput, port: &MidiInputPort) {
    //input.connect(&port)
}