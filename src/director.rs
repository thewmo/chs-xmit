use std::path::PathBuf;
use crossbeam_channel::Receiver;
use crossbeam_channel::RecvTimeoutError;
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
use crate::show::{ShowDefinition,LightMapping,MidiMappingType,LightMappingType,Effect};
use crate::packet::{Command,ShowPacket,PacketPayload,Packet};

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

    // reference to the radio
    pub radio: &'a Radio,

    /// the current show tempo
    pub tempo: u8,

    /// the moment of the last midi event we cared about
    pub last_midi: Instant,

    /// group name to id lookup
    pub groups: HashMap<String, u8>,

    // lookup from group name to group member ids
    pub group_members: HashMap<String, Vec<u8>>,

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

/// in JSON we represent time as milliseconds, but the radio format is a bit tricker to save space
/// attack and decay values less then 1.279 seconds are sent in units of hundredths of a second,
/// while values greaten than that are sent in tenths of seconds (idea being the resolution matters
/// less the longer the attack or decay actually is)
fn convert_millis_adr(millis: u32) -> u8 {
    match millis {
        0..=1279 => ((millis / 10) & 0x7F) as u8,
        _ => (((millis / 100) & 0x7F) | 0x80) as u8
    }
}

/// sustain is sent in tenths of seconds up until 12.799 seconds, then whole seconds after that
fn convert_millis_sustain(millis: u32) -> u8 {
    match millis {
        0..=12799 => ((millis / 100) & 0x7F) as u8,
        _ => (((millis / 1000) & 0x7F) | 0x80) as u8
    }
}

impl<'a> ShowState<'a> {
    pub fn new(show: &ShowDefinition, radio: &'a Radio) -> ShowState<'a> {
        let mut groups: HashMap<String,u8> = HashMap::new();
        let mut group_members: HashMap<String,Vec<u8>> = HashMap::new();
        let mut group_id = 11;
        let mut note_mappings: HashMap<(u4,u7),usize> = HashMap::new();
        let mut controller_mappings: HashMap<(u4,u7),usize> = HashMap::new();
        for r in show.receivers.iter() {
            if let Some(group_name) = &r.group_name {
                if !groups.contains_key(group_name) {
                    groups.insert(group_name.clone(), group_id);
                    group_id = group_id + 1;
                }
                group_members.entry(group_name.clone()).or_insert_with(Vec::new).push(r.id);
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
            last_midi: Instant::now(),
            groups,
            group_members,
            note_mappings, 
            controller_mappings, 
            receiver_state: vec![ReceiverState::new(); mapping_count] }
    }
    
    /// Send control packets to all the receivers telling them
    /// what group they're in and how many leds they have
    fn configure_receivers(self: &Self, show: &ShowDefinition) -> Result<(), RadioError> {
        for receiver in show.receivers.iter() {

            if let Some(group_name) = &receiver.group_name {
                self.radio.send(&Packet {
                    recipients: vec![receiver.id],
                    payload: PacketPayload::Control(
                        Command::SetGroup { group_id: 
                            *self.groups.get(group_name).unwrap() })
                })?;
            }
            self.radio.send(&Packet {
                recipients: vec![receiver.id],
                payload: PacketPayload::Control(
                    Command::SetLedCount { led_count: receiver.led_count })
            })?;

            info!("Configured receiver: {} with group id: {} and led count: {}", 
            receiver.id, receiver.group_name.as_ref().map_or("none", |g| g.as_str()), receiver.led_count);
        }

        // now send a reset packet to all receivers
        self.radio.send(&Packet { 
            recipients: vec![0],
            payload: PacketPayload::Control(Command::Reset)
        })?;

        Ok(())
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
                    _ => Ok(DEFAULT_TICK)
                }
            }
            _ => Ok(DEFAULT_TICK)
        };
        next_event
    }

    fn process_controller(self: &mut Self, show: &ShowDefinition, channel: u4, controller: u7, value: u7) -> anyhow::Result<Duration> {
        let mapping = self.controller_mappings.get(&(channel, controller));
        if let Some(index) = mapping {
            self.last_midi = Instant::now();
            let light_mapping = show.mappings.get(*index).unwrap();
            if value == 127 {
                self.activate(show, &light_mapping)?;
            } else {
                self.deactivate(show, &light_mapping);
            }
        }
        Ok(DEFAULT_TICK)
    }

    fn process_note_on(self: &mut Self, show: &ShowDefinition, channel: u4, key: u7, _velocity: u7) -> anyhow::Result<Duration> {
        let mapping = self.note_mappings.get(&(channel, key));
        if let Some(index) = mapping {
            self.last_midi = Instant::now();
            let light_mapping = show.mappings.get(*index).unwrap();
            self.activate(show, &light_mapping)?;
        }
        Ok(DEFAULT_TICK)
    }

    fn process_note_off(self: &mut Self, show: &ShowDefinition, channel: u4, key: u7, _velocity: u7) -> anyhow::Result<Duration> {
        let mapping = self.note_mappings.get(&(channel, key));
        if let Some(index) = mapping {
            self.last_midi = Instant::now();
            let light_mapping = show.mappings.get(*index).unwrap();
            self.deactivate(&show, &light_mapping);
        }
        Ok(DEFAULT_TICK)
    }

    fn activate(self: &mut Self, show: &ShowDefinition, light_mapping: &LightMapping) -> anyhow::Result<Duration> {
        match &light_mapping.light {
            LightMappingType::Effect(effect) => self.activate_effect(&show, &light_mapping, &effect),
            LightMappingType::Clip(clip) => self.activate_clip(&show, &light_mapping, &clip)
        }
    }

    fn activate_effect(self: &mut Self, show: &ShowDefinition, light_mapping: &LightMapping, effect: &Effect)-> anyhow::Result<Duration> {
        self.tempo = light_mapping.tempo.unwrap_or(self.tempo);
        let mut show_packet = ShowPacket {
            effect: effect.to_effect_id(),
            color: *show.colors.get(&light_mapping.color).unwrap(),
            attack: convert_millis_adr(light_mapping.attack),
            sustain: convert_millis_sustain(light_mapping.sustain),
            release: convert_millis_adr(light_mapping.release),
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
        self.radio.send(&packet)?;
//        light_mapping.targets.iter().flat_map(|i| self.groups.get(i));
        Ok(DEFAULT_TICK)
    }

    fn process_tick(self: &Self) -> anyhow::Result<Duration> {
        //debug!("Tick...");
        // TODO - implement the lights-out packet logic
        Ok(DEFAULT_TICK)
    }

    fn activate_clip(self: &mut Self, _show: &ShowDefinition, _light_mapping: &LightMapping, _clip: &str) -> anyhow::Result<Duration> {
        Ok(DEFAULT_TICK)
    }

    fn deactivate(self: &mut Self, _show: &ShowDefinition, light_mapping: &LightMapping) {
        match &light_mapping.light {
            LightMappingType::Effect(e) => { self.deactivate_effect(light_mapping, e); }
            LightMappingType::Clip(c) => { self.deactivate_clip(light_mapping,c); }
        }
    }

    fn deactivate_effect(self: &mut Self, _light_mapping: &LightMapping, _effect: &Effect) {
    }

    fn deactivate_clip(self: &mut Self, _light_mapping: &LightMapping, _clip: &str) {

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
        debug!("Entering run_show");
        let show_path = PathBuf::from(&self.config.show_file);
        debug!("Show path is: {:?}", show_path);
        'exit: loop {
            let file = File::open(&show_path)?;
            debug!("Opened file");
            let show = match serde_json::from_reader(&file) {
                Err(e) => { error!("Error parsing show JSON: {:?}", e); None },
                Ok(show) => { info!("Loaded show: {:?}", show); Some(show) }
            };
            'load: loop {

                let mut state = show.as_ref().map(|s| { ShowState::new(s, &self.radio) });
                match &mut state {
                    Some(s) => s.configure_receivers(show.as_ref().unwrap()),
                    _ => Ok(())
                }?;
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
                                    timeout = match &mut state {
                                        Some(s) => s.process_midi(show.as_ref().unwrap(), ts, buf)?,
                                        _ => DEFAULT_TICK
                                    }
                                }
                            }
                        }
                        Err(e) => match e {
                            RecvTimeoutError::Timeout => {
                                timeout = state.as_mut().map_or(DEFAULT_TICK, |s| s.process_tick().unwrap());
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
        debug!("Exiting run_show");
        Ok(())
    }
}