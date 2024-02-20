use midir::{MidiInput, MidiInputPort, MidiOutput, MidiOutputPort};
use crate::config::ConfigFile;

pub fn midi_init(config: &ConfigFile) -> Result<(MidiInput, MidiOutput), midir::InitError> {
    Ok((MidiInput::new(&config.midi_client_name)?, MidiOutput::new(&config.midi_client_name)?))
}

/// enumerate the midi ports available on the system
pub fn midi_enum(input: &MidiInput) {
    println!("Available Midi Ports");
    println!("====================");
    for (i, p) in input.ports().iter().enumerate() {
        println!("{}: {}", i+1, input.port_name(p).unwrap());
    }
}

pub fn find_ports(input: &MidiInput, output: &MidiOutput, port_prefix: &str) -> Option<(MidiInputPort,MidiOutputPort)> {
    let input_ports = input.ports();
    let in_port_option = input_ports.into_iter().find(|p| 
        input.port_name(p).unwrap().starts_with(&port_prefix));

    let output_ports = output.ports();
    let out_port_option = output_ports.into_iter().find(|p| 
        output.port_name(p).unwrap().starts_with(&port_prefix));

    if in_port_option.is_some() && out_port_option.is_some() {
        Some((in_port_option.unwrap(), out_port_option.unwrap()))
    } else { None }
}
