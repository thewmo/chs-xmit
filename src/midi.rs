use midir::{MidiInput, MidiInputPort};
use crate::config::ConfigFile;

pub fn midi_init(config: &ConfigFile) -> Result<MidiInput, midir::InitError> {
    MidiInput::new(&config.midi_client_name)
}

/// enumerate the midi ports available on the system
pub fn midi_enum(input: &MidiInput) {
    println!("Available Midi Ports");
    println!("====================");
    for (i, p) in input.ports().iter().enumerate() {
        println!("{}: {}", i+1, input.port_name(p).unwrap());
    }
}

pub fn find_port(input: &MidiInput, port_prefix: &str) -> Option<MidiInputPort> {
    let ports = input.ports();
    let port_option = ports.iter().find(|p| 
        input.port_name(p).unwrap().starts_with(&port_prefix));
    port_option.map(|p| p.clone())    
}
