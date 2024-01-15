use std::error::Error;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::io;
use std::fs;

use crate::config::ConfigFile;
use crate::packet::Command;
use crate::radio::Radio;
use crate::show::{Show, ReceiverConfiguration};
use crate::packet::{ControlPacket,ShowPacket};


/// This module is where a lot of the action happens. MIDI message
/// meet show configuration to fire radio packets.

pub enum DirectorMessage {
    MidiMessage { ts: u64, buf: Vec<u8> },
    Shutdown,
}

pub struct Director<'a> {
    pub config: &'a ConfigFile,
    pub radio: &'a mut Radio,
    pub rx: Receiver<DirectorMessage>
}

impl<'a> Director<'a> {

    pub fn run_show(self: &mut Self) -> Result<(),Box<dyn Error>> {

        let show_path = PathBuf::from(&self.config.show_file);
        let show_def = Director::load_show(&show_path)?;
        self.configure_receivers(&show_def.receivers);
        Ok(())
    }

    fn load_show(show_path: &PathBuf) -> Result<Show, io::Error> {
        let file = fs::File::open(&show_path)?;
        Ok(serde_json::from_reader(&file)?)
    }

    fn configure_receivers(self: &mut Self, receivers: &[ReceiverConfiguration]) -> Result<(), RadioError> {
        for receiver in receivers {
            let set_group_packet = Command::SetGroup { }
        }
    }


}