use std::path::PathBuf;
use crossbeam_channel::Receiver;
use crossbeam_channel::RecvTimeoutError;
use std::io;
use std::fs;
use std::collections::HashMap;
use log::{debug,info,error};
use std::time::Duration;

use crate::config::ConfigFile;
use crate::radio::{Radio,RadioError};
use crate::show::{Show, ReceiverConfiguration};
use crate::packet::{ControlPacket,ShowPacket,PacketPayload,Packet,CommandId};


/// This module is where a lot of the action happens. MIDI message
/// meet show configuration to fire radio packets.

const DEFAULT_TICK: Duration = Duration::from_secs(1);

pub enum DirectorMessage {
    MidiMessage { ts: u64, buf: Vec<u8> },

    /// shut down the event loop and exit the run_show routine
    Shutdown,

    /// reload the show config and then reinitialize receivens
    Reload,

    /// just reinitialize receivers
    ReInitialize
}

struct ShowMetadata {
    pub show: Show,
    pub groups: HashMap<String, u8>
}

/// tracks the last instruction sent to a particular receiver, so
/// we know what it's doing
#[derive(Clone,Copy)]
struct ReceiverState {

}

pub struct Director<'a> {
    pub config: &'a ConfigFile,
    pub radio: &'a mut Radio,
    pub rx: Receiver<DirectorMessage>,
    receiver_state: [ReceiverState;255]
}

impl<'a> Director<'a> {

    pub fn new(config: &'a ConfigFile, radio: &'a mut Radio, rx: Receiver<DirectorMessage>) -> Director<'a> {
        Director {
            config,
            radio,
            rx,
            receiver_state: [ReceiverState {};255]
        }
    }

    pub fn run_show(self: &mut Self) -> anyhow::Result<()> {
        let show_path = PathBuf::from(&self.config.show_file);
        'exit: loop {
            let show_def = self.load_show(&show_path)?;
            info!("Loaded show: {:?}", show_def.show);
            'load: loop {
                self.configure_receivers(&show_def)?;
                info!("Initialized receivers");
                let mut timeout = DEFAULT_TICK;
                'init: loop {
                    match self.rx.recv_timeout(timeout) {
                        Ok(message) => {
                            match message {
                                DirectorMessage::ReInitialize => break 'init,
                                DirectorMessage::Reload => break 'load,
                                DirectorMessage::Shutdown => break 'exit,
                                DirectorMessage::MidiMessage { ts, buf } => {
                                    timeout = self.process_midi(ts, buf)?;
                                }
                            }
                        }
                        Err(e) => match e {
                            RecvTimeoutError::Timeout => {
                                timeout = self.process_tick()?;
                            },
                            RecvTimeoutError::Disconnected => {
                                error!("Channel closed, exiting show loop");
                                break 'exit;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn process_midi(self: &mut Self, _ts: u64, buf: Vec<u8>) -> anyhow::Result<Duration> {
        let midi_event = midly::live::LiveEvent::parse(&buf)?;
        debug!("Received MIDI event: {:?}", midi_event);
        Ok(DEFAULT_TICK)
    }

    fn process_tick(self: &mut Self) -> anyhow::Result<Duration> {
        debug!("Tick...");
        Ok(DEFAULT_TICK)
    }

    /// Load the show from the specified show JSON path, and derive anything
    /// that needs to be derived
    fn load_show(self: &mut Self, show_path: &PathBuf) -> Result<ShowMetadata, io::Error> {
        let file = fs::File::open(&show_path)?;
        let show: Show = serde_json::from_reader(&file)?;
        // build a map from group name to group id
        let mut groups: HashMap<String,u8> = HashMap::new();
        let mut group_id = 11;
        for r in show.receivers.iter() {
            if !groups.contains_key(&r.group_name) {
                groups.insert(r.group_name.clone(), group_id);
                group_id = group_id + 1;
            }
        }
        Ok(ShowMetadata { show, groups })
    }

    /// Send control packets to all the receivers telling them
    /// what group they're in and how many leds they have
    fn configure_receivers(self: &mut Self, show_metadata: &ShowMetadata) -> Result<(), RadioError> {
        for receiver in show_metadata.show.receivers.iter() {
            let set_group_packet = ControlPacket {
                command_id: CommandId::SetGroup,
                param1: *show_metadata.groups.get(&receiver.group_name).unwrap(),
                param2: 0,
                request_reply: false
            };
            let set_leds_packet = ControlPacket {
                command_id: CommandId::SetLedCount,
                param1: (receiver.led_count >> 8) as u8,
                param2: (receiver.led_count & 0xFFu16) as u8,
                request_reply: false
            };
            let mut packet = Packet {
                recipients: vec![receiver.id],
                payload: PacketPayload::Control(set_group_packet)
            };
            self.radio.send(&packet)?;
            packet.payload = PacketPayload::Control(set_leds_packet);
            self.radio.send(&packet)?;

            info!("Configured receiver: {} with group id: {} and led count: {}", receiver.id, set_group_packet.param1, receiver.led_count);
        }
        Ok(())
    }


}