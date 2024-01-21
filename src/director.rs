use std::path::PathBuf;
use crossbeam_channel::Receiver;
use crossbeam_channel::RecvTimeoutError;
use std::io;
use std::fs::File;
use std::collections::HashMap;
use log::{debug,info,error};
use std::time::{Duration,Instant};
use midly::live::LiveEvent;
use midly::MidiMessage;
use midly::num::{u4,u7};
use musical_note::ResolvedNote;

use crate::config::ConfigFile;
use crate::packet::EffectId;
use crate::radio::{Radio,RadioError};
use crate::show::{ShowDefinition,ReceiverConfiguration,LightMapping,MidiMappingType,LightMappingType,Effect};
use crate::packet::{Command,ShowPacket,PacketPayload,Packet,CommandId};

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

/// mutable state associated with the show. some things are derived from
/// the show json, other things (eg receiver and clip state) continuously
/// change as the show is performed
struct ShowState<'a> {

    pub radio: &'a Radio,

    /// the current show tempo
    pub tempo: u8,

    /// the moment of the last activating packet
    pub last_on: Instant,

    /// group name to id lookup
    pub groups: HashMap<String, u8>,

    /// midi channel/note to note mapping lookup
    pub note_mappings: HashMap<(u4,u7), usize>,

    /// midi channel/cc to note mapping lookup
    pub controller_mappings: HashMap<(u4,u7), usize>,

    /// records what all the receivers are doing at each show moment
    pub receiver_state: Vec<ReceiverState>
}

/// tracks the last instruction sent to a particular receiver, so
/// we know what it's doing
#[derive(Clone,Copy)]
struct ReceiverState {
    pub effect: Option<EffectId>,
    pub trigger_mapping: usize
}

impl ReceiverState {
    pub fn new() -> Self {
        Self {
            effect: None,
            trigger_mapping: 0
        }
    }

    pub fn reset(self: &mut Self) {
        self.effect = None;
        self.trigger_mapping = 0;
    }
}

impl<'a> ShowState<'a> {
    pub fn new(show: &ShowDefinition, radio: &'a Radio) -> ShowState<'a> {
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
                    note_mappings.insert(((*channel).into(), ResolvedNote::from_str(note).unwrap().midi.into()), i);
                }
                MidiMappingType::Controller { channel, cc } => {
                    controller_mappings.insert(((*channel).into(), (*cc).into()), i);
                }
            }
        }
        let mapping_count = show.mappings.len();
        ShowState { 
            radio,
            tempo: 0, 
            last_on: Instant::now(),
            groups, 
            note_mappings, 
            controller_mappings, 
            receiver_state: vec![ReceiverState::new(); mapping_count] }
    }
    
    /// Send control packets to all the receivers telling them
    /// what group they're in and how many leds they have
    fn configure_receivers(self: &Self, show: &ShowDefinition) -> Result<(), RadioError> {
        for receiver in show.receivers.iter() {

            self.radio.send(&Packet {
                recipients: vec![receiver.id],
                payload: PacketPayload::Control(
                    Command::SetGroup { group_id: 
                        *self.groups.get(&receiver.group_name).unwrap() })
            })?;
            self.radio.send(&Packet {
                recipients: vec![receiver.id],
                payload: PacketPayload::Control(
                    Command::SetLedCount { led_count: receiver.led_count })
            })?;

            info!("Configured receiver: {} with group id: {} and led count: {}", 
            receiver.id, receiver.group_name, receiver.led_count);
        }

        // now send a reset packet to all receivers
        self.radio.send(&Packet { 
            recipients: vec![0],
            payload: PacketPayload::Control(Command::Reset)
        })?;

        Ok(())
    }


    pub fn reset(self: &mut Self) {
        self.receiver_state.iter_mut().for_each(|r| r.reset());
        self.tempo = 0;
    }
    
    pub fn process_midi(self: &mut Self, show: &ShowDefinition, _ts: u64, buf: Vec<u8>) -> anyhow::Result<Duration> {
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

    fn process_controller(self: &mut Self, show: &ShowDefinition, channel: u4, controller: u7, value: u7) -> Duration {
        let mapping = self.controller_mappings.get(&(channel, controller));
        if let Some(index) = mapping {
            let light_mapping = show.mappings.get(*index).unwrap();
            if value == 127 {
                self.activate(show, &light_mapping);
            } else {
                self.deactivate(show, &light_mapping);
            }
        }
        DEFAULT_TICK
    }

    fn process_note_on(self: &mut Self, show: &ShowDefinition, channel: u4, key: u7, velocity: u7) -> Duration {
        let mapping = self.note_mappings.get(&(channel, key));
        if let Some(index) = mapping {
            let light_mapping = show.mappings.get(*index).unwrap();
            self.activate(show, &light_mapping);
        }
        DEFAULT_TICK
    }

    fn process_note_off(self: &mut Self, show: &ShowDefinition, channel: u4, key: u7, velocity: u7) -> Duration {
        let mapping = self.note_mappings.get(&(channel, key));
        if let Some(index) = mapping {
            let light_mapping = show.mappings.get(*index).unwrap();
            self.deactivate(&show, &light_mapping);
        }
        DEFAULT_TICK
    }

    fn activate(self: &mut Self, show: &ShowDefinition, light_mapping: &LightMapping) {
        match &light_mapping.light {
            LightMappingType::Effect(effect) => self.activate_effect(&show, &light_mapping, &effect),
            LightMappingType::Clip(clip) => self.activate_clip(&show, &light_mapping, &clip)
        }
    }

    fn activate_effect(self: &mut Self, show: &ShowDefinition, light_mapping: &LightMapping, effect: &Effect) {
        let mut show_packet = ShowPacket {
            effect: effect.to_effect_id(),
            color: *show.colors.get(&light_mapping.color).unwrap(),
            attack: light_mapping.attack as u8,
            sustain: light_mapping.sustain as u8,
            release: light_mapping.release as u8,
            param1: 0,
            param2: 0,
            tempo: self.tempo
        };
        effect.populate_effect_params(&mut show_packet);
        let packet = Packet {
            // this monstrosity converts targets to group ids, if they are group ids, but otherwise converts them to u8s
            recipients: light_mapping.targets.iter()
                .map(|tgt| self.groups.get(tgt)
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

    fn activate_clip(self: &mut Self, show: &ShowDefinition, light_mapping: &LightMapping, clip: &str) {

    }

    fn deactivate(self: &mut Self, show: &ShowDefinition, light_mapping: &LightMapping) {

    }

}

pub struct Director {
    config: ConfigFile,
    radio: Radio,
    rx: Receiver<DirectorMessage>
}

impl Director {

    pub fn new(config: ConfigFile, radio: Radio, rx: Receiver<DirectorMessage>) -> Director {
        Director {
            config,
            radio,
            rx
        }
    }

    pub fn run_show(self: &mut Self) -> anyhow::Result<()> {
        let show_path = PathBuf::from(&self.config.show_file);
        'exit: loop {
            let show: ShowDefinition = serde_json::from_reader(&File::open(&show_path)?)?;
            info!("Loaded show: {:?}", show);
            'load: loop {
                let mut state = ShowState::new(&show, &self.radio);
                state.configure_receivers(&show)?;
                info!("Reset receivers and show state");
                let mut timeout = DEFAULT_TICK;
                'init: loop {
                    match self.rx.recv_timeout(timeout) {
                        Ok(message) => {
                            match message {
                                DirectorMessage::ReInitialize => break 'init,
                                DirectorMessage::Reload => break 'load,
                                DirectorMessage::Shutdown => break 'exit,
                                DirectorMessage::MidiMessage { ts, buf } => {
                                    timeout = state.process_midi(&show, ts, buf)?;
                                }
                            }
                        }
                        Err(e) => match e {
                            RecvTimeoutError::Timeout => {
                                timeout = state.process_tick()?;
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



}