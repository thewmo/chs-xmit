use std::path::PathBuf;
use crossbeam_channel::Receiver;
use crossbeam_channel::RecvTimeoutError;
use std::io;
use std::fs;
use std::collections::HashMap;
use log::{debug,info,error};
use std::time::Duration;
use midly::live::LiveEvent;
use midly::MidiMessage;
use midly::num::{u4,u7};

use crate::config::ConfigFile;
use crate::radio::{Radio,RadioError};
use crate::show::{Show,ReceiverConfiguration,LightMapping,MidiMappingType,LightMappingType,Effect};
use crate::packet::{ControlPacket,ShowPacket,PacketPayload,Packet,CommandId};


/// This module is where a lot of the action happens. MIDI message
/// meet show configuration to fire radio packets.

const DEFAULT_TICK: Duration = Duration::from_secs(1);

pub enum DirectorMessage {
    /// deliver a payload of a midi event
    MidiMessage { ts: u64, buf: Vec<u8> },

    /// shut down the event loop and exit the run_show routine
    Shutdown,

    /// reload the show config and then reinitialize receivens
    Reload,

    /// just reinitialize receivers
    ReInitialize
}

/// a wrapper around the show data loaded from JSON that provides
/// lookup facilities for mappings and groups
struct ShowData {
    pub show: Show,

    /// group name to id lookup
    pub groups: HashMap<String, u8>,

    /// midi channel/note to note mapping lookup
    pub note_mappings: HashMap<(u4,u7), usize>,

    /// midi channel/cc to note mapping lookup
    pub controller_mappings: HashMap<(u4,u7), usize>,
}

/// tracks the last instruction sent to a particular receiver, so
/// we know what it's doing
#[derive(Clone,Copy)]
struct ReceiverState {

}

pub struct Director {
    config: ConfigFile,
    radio: Radio,
    rx: Receiver<DirectorMessage>,
    receiver_state: [ReceiverState;255],
}

impl Director {

    pub fn new(config: ConfigFile, radio: Radio, rx: Receiver<DirectorMessage>) -> Director {
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
            let show = self.load_show(&show_path)?;
            info!("Loaded show: {:?}", show.show);
            'load: loop {
                self.configure_receivers(&show)?;
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
                                    timeout = self.process_midi(&show, ts, buf)?;
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

    fn process_midi(self: &Self, show: &ShowData, _ts: u64, buf: Vec<u8>) -> anyhow::Result<Duration> {
        let midi_event = midly::live::LiveEvent::parse(&buf)?;
        debug!("Received MIDI event: {:?}", midi_event);
        let next_event = match midi_event {
            LiveEvent::Midi { channel, message } => {
                match message {
                    MidiMessage::Controller { controller, value } => {
                        self.process_controller(show, channel, controller, value)
                    },
                    MidiMessage::NoteOn { key, vel } => {
                        self.process_note_on(show, channel, key, vel)
                    },
                    MidiMessage::NoteOff { key, vel } => {
                        self.process_note_off(show, channel, key, vel)
                    },
                    _ => DEFAULT_TICK
                }
            }
            _ => DEFAULT_TICK
        };
        Ok(next_event)
    }

    fn process_controller(self: &Self, show: &ShowData, channel: u4, controller: u7, value: u7) -> Duration {
        let mapping = show.controller_mappings.get(&(channel, controller));
        if let Some(index) = mapping {
            let light_mapping = show.show.mappings.get(*index).unwrap();
            if value == 127 {
                self.activate(show, light_mapping);
            } else {
                self.deactivate(show, light_mapping);
            }
        }
        DEFAULT_TICK
    }

    fn process_note_on(self: &Self, show: &ShowData, channel: u4, key: u7, velocity: u7) -> Duration {
        let mapping = show.note_mappings.get(&(channel, key));
        if let Some(index) = mapping {
            let light_mapping = show.show.mappings.get(*index).unwrap();
            self.activate(show, light_mapping);
        }
        DEFAULT_TICK
    }

    fn process_note_off(self: &Self, show: &ShowData, channel: u4, key: u7, velocity: u7) -> Duration {
        let mapping = show.note_mappings.get(&(channel, key));
        if let Some(index) = mapping {
            let light_mapping = show.show.mappings.get(*index).unwrap();
            self.deactivate(show, light_mapping);
        }
        DEFAULT_TICK
    }

    fn activate(self: &Self, show: &ShowData, light_mapping: &LightMapping) {
        match &light_mapping.light {
            LightMappingType::Effect(effect) => self.activate_effect(show, light_mapping, effect),
            LightMappingType::Clip(clip) => self.activate_clip(light_mapping, &clip)
        }
    }

    fn activate_effect(self: &Self, show: &ShowData, light_mapping: &LightMapping, effect: &Effect) {
        let show_packet = ShowPacket {
            effect: effect.to_effect_id(),
            color: light_mapping.color,
            attack: light_mapping.attack as u8,
            sustain: light_mapping.sustain as u8,
            release: light_mapping.release as u8,
            param1: 0,
            param2: 0,
            tempo: 0
        };
        let packet = Packet {
            // this monstrosity converts targets to group ids, if they are group ids, but otherwise converts them to u8s
            recipients: light_mapping.targets.iter()
                .map(|tgt| show.groups.get(tgt)
                    .map(|gid| *gid)
                    .or_else(|| tgt.parse::<u8>().ok()).unwrap())
                .collect(),
            payload: PacketPayload::Show(show_packet),
        };
        self.radio.send(&packet);
    }

    fn process_tick(self: &Self) -> anyhow::Result<Duration> {
        debug!("Tick...");
        Ok(DEFAULT_TICK)
    }

    fn activate_clip(self: &Self, mapping: &LightMapping, clip: &str) {

    }

    fn deactivate(self: &Self, show: &ShowData, mapping: &LightMapping) {

    }

    /// Load the show from the specified show JSON path, and derive anything
    /// that needs to be derived
    fn load_show(self: &Self, show_path: &PathBuf) -> Result<ShowData, io::Error> {
        let file = fs::File::open(&show_path)?;
        let show: Show = serde_json::from_reader(&file)?;
        // build a map from group name to group id
        let mut groups: HashMap<String,u8> = HashMap::new();
        let mut group_id = 11;
        let mut note_mappings: HashMap<(u4,u7),usize> = HashMap::new();
        let mut controller_mappings: HashMap<(u4,u7),usize> = HashMap::new();
        for r in show.receivers.iter() {
            if !groups.contains_key(&r.group_name) {
                groups.insert(r.group_name.clone(), group_id);
                group_id = group_id + 1;
            }
        }
        for (i, m) in show.mappings.iter().enumerate() {
            match &m.midi {
                MidiMappingType::Note { channel, note } => {
                    note_mappings.insert(((*channel).into(), Self::xlat(&note)), i);
                }
                MidiMappingType::Controller { channel, cc } => {
                    controller_mappings.insert(((*channel).into(), (*cc).into()), i);
                }
            }
        }
        Ok(ShowData { show, groups, note_mappings, controller_mappings })
    }

    fn xlat(note: &str) -> u7 {
        0.into()
    }

    /// Send control packets to all the receivers telling them
    /// what group they're in and how many leds they have
    fn configure_receivers(self: &Self, show: &ShowData) -> Result<(), RadioError> {
        for receiver in show.show.receivers.iter() {
            let set_group_packet = ControlPacket {
                command_id: CommandId::SetGroup,
                param1: *show.groups.get(&receiver.group_name).unwrap(),
                param2: 0,
                request_reply: false
            };
            let set_leds_packet = ControlPacket {
                command_id: CommandId::SetLedCount,
                param1: (receiver.led_count >> 8) as u8,
                param2: (receiver.led_count & 0xFF) as u8,
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