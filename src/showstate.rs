use log::{debug,info};
use std::rc::Rc;
use std::time::{Duration,Instant};
use std::collections::HashMap;
use std::cell::RefCell;
use midly::live::LiveEvent;
use midly::MidiMessage;
use midly::num::{u4,u7};
use musical_note::ResolvedNote;
use anyhow::{Result, anyhow};

use crate::radio::{Radio,RadioError};
use crate::show::{ClipStep, Effect, LightMapping, LightMappingType, MidiMappingType, ShowDefinition};
use crate::packet::{Command, Packet, PacketPayload, ShowPacket, GROUP_ID_RANGE};
use crate::clip::ClipState;
use crate::director::DEFAULT_TICK;

/// mutable state associated with the show. some things are derived from
/// the show json, other things (eg receiver and clip state) continuously
/// change as the show is performed
/// 
/// lifetimes here - 'a is the director lifetime, 'b is the show lifetime
pub struct ShowState<'a,'b> {

    // reference to the radio
    radio: &'a Radio,

    /// the moment of the last midi event we cared about
    pub last_midi: Instant,

    /// a map to lookup the u8 ids for named targets
    target_lookup: HashMap<String,u8>,

    /// midi channel/note to light mapping key
    note_mappings: HashMap<(u4,u7), usize>,

    /// midi channel/cc to light mapping key
    controller_mappings: HashMap<(u4,u7), usize>,

    light_mappings: HashMap<usize,LightMappingMeta<'b>>,

    clip_state: HashMap<String,ClipState>,

}

/// tracks the last instruction sent to a particular receiver, so
/// we know what it's doing
#[derive(Clone,Copy)]
struct ReceiverState {
    pub id: u8,
    trigger_mapping: usize
}

impl ReceiverState {
    const INACTIVE: usize = 0;

    pub fn new(id: u8) -> Self {
        Self {
            id,
            trigger_mapping: Self::INACTIVE
        }
    }

    pub fn activate(self: &mut Self, mapping: &LightMapping) {
        self.trigger_mapping = match mapping {
            _ if !mapping.one_shot.unwrap_or(false) => mapping.get_id(),
            _ => Self::INACTIVE
        }
    }

    pub fn activated_by(self: &Self, mapping: &LightMapping) -> bool {
        let mapping_id = mapping.get_id();
        self.trigger_mapping == mapping_id
    }

    pub fn deactivate(self: &mut Self, mapping: &LightMapping) -> bool {
        let result = self.trigger_mapping == mapping.get_id();
        if result {
            self.trigger_mapping = Self::INACTIVE;
        }
        result
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
/// sustain of zero means "on until an off command"
fn convert_millis_sustain(millis: u32) -> u8 {
    match millis {
        0 => 255, 
        1..=12799 => ((millis / 100) & 0x7F) as u8,
        _ => (((millis / 1000) & 0x7F) | 0x80) as u8
    }
}

/// a wrapper around a light mapping that stashes a reference to the source mapping,
/// and the resolved target vector for packets, as well as a vector to references
/// to all the receiver state instances to update when the mapping is triggered
struct LightMappingMeta<'a> {
    pub source: &'a LightMapping,
    pub targets: Vec<u8>,
    pub receivers: Vec<Rc<RefCell<ReceiverState>>>
}

/// given a target expressed as a json node of any type, convert
/// it to a string that represents either a u8 or a named receiver,
/// or return an error if the node is not of a type that con be so converted
fn convert_target(json_value: &serde_json::Value) -> Result<String> {
    match &json_value {
        serde_json::Value::Number(value) => 
            value.as_u64().and_then(|n| match n {
                1..=255 => Some(n.to_string()),
                _ => None
            }).ok_or_else(|| anyhow!("Number in target list must be receiver id in range (1, 255): {}", value)),
        serde_json::Value::String(value) => Ok(value.to_owned()),
        _ => Err(anyhow!("Unsupported data type in target list: {}", json_value))
    }
}

impl<'a,'b> ShowState<'a,'b> {
    pub fn new(show: &'b ShowDefinition, radio: &'a Radio) -> Result<ShowState<'a,'b>> {

        let mut target_lookup: HashMap<String,u8> = HashMap::new();
        let mut group_members: HashMap<u8,Vec<u8>> = HashMap::new();
        let mut group_id = GROUP_ID_RANGE.start;
        let mut light_mappings: HashMap<usize, LightMappingMeta> = HashMap::new();
        let mut note_mappings: HashMap<(u4,u7), usize> = HashMap::new();
        let mut controller_mappings: HashMap<(u4,u7), usize> = HashMap::new();
        let mut receiver_state: HashMap<u8,Rc<RefCell<ReceiverState>>> = HashMap::new();
        let mut clip_state: HashMap<String,ClipState> = HashMap::new();

        // preprocess receivers
        for r in show.receivers.iter() {
            // update the target lookup map
            target_lookup.insert(r.id.to_string(), r.id);
            if let Some(receiver_name) = &r.name {
                target_lookup.insert(receiver_name.clone(), r.id);
            }
            // if the receiver is a group member, add it to the group
            if let Some(group_name) = &r.group_name {
                if !target_lookup.contains_key(group_name) {
                    target_lookup.insert(group_name.clone(), group_id);
                    group_id = group_id + 1;
                }
                let group_id = target_lookup.get(group_name).unwrap();
                group_members.entry(*group_id).or_insert_with(Vec::new).push(r.id);
            }
            // create a reference-counted receiver state entry for the receiver
            receiver_state.insert(r.id, Rc::new(RefCell::new(ReceiverState::new(r.id))));
        }
        
        // preprocess light mappings
        for m in show.mappings.iter() {
            light_mappings.insert(m.get_id(), 
                ShowState::create_light_mapping_meta(m, &target_lookup, &group_members, &receiver_state)?);
            match &m.midi {
                MidiMappingType::Note { channel, note } => {
                    note_mappings.insert(((*channel).into(), ResolvedNote::from_str(&note).unwrap().midi.into()), 
                        m.get_id());
                }
                MidiMappingType::Controller { channel, cc } => {
                    controller_mappings.insert(((*channel).into(), (*cc).into()), 
                        m.get_id());
                }
            }
        }

        // preprocess clip-embedded light mappings
        for clip_steps in show.clips.values() {
            for step in clip_steps.iter() {
                match step {
                    ClipStep::MappingOn(m) => {
                        light_mappings.insert(m.get_id(), 
                            ShowState::create_light_mapping_meta(m, &target_lookup, &group_members, &receiver_state)?);
                    },
                    _ => {}
                }
            }
        }

        Ok(ShowState { 
            radio,
            last_midi: Instant::now(),
            target_lookup,
            note_mappings, 
            controller_mappings,
            light_mappings,
            clip_state
     })
    }
    
    /// Send control packets to all the receivers telling them
    /// what group they're in and how many leds they have
    pub fn configure_receivers(self: &Self, show: &ShowDefinition) -> Result<(), RadioError> {
        for receiver in show.receivers.iter() {

            if let Some(group_name) = &receiver.group_name {
                self.radio.send(&Packet {
                    recipients: &vec![receiver.id],
                    payload: PacketPayload::Control(
                        Command::SetGroup { group_id: 
                            *self.target_lookup.get(group_name).unwrap() })
                })?;
            }
            self.radio.send(&Packet {
                recipients: &vec![receiver.id],
                payload: PacketPayload::Control(
                    Command::SetLedCount { led_count: receiver.led_count })
            })?;

            info!("Configured receiver: {} with group id: {} and led count: {}", 
            receiver.id, receiver.group_name.as_ref().map_or("none", |g| g.as_str()), receiver.led_count);
        }

        // now send a reset packet to all receivers
        self.radio.send(&Packet { 
            recipients: &vec![],
            payload: PacketPayload::Control(Command::Reset)
        })?;

        Ok(())
    }
    
    pub fn process_midi(self: &mut Self, show: &ShowDefinition, _ts: u64, buf: Vec<u8>) -> anyhow::Result<Duration> {
        let midi_event = midly::live::LiveEvent::parse(&buf)?;
        debug!("Received MIDI event: {:?}", midi_event);
        let next_event = match midi_event {
            LiveEvent::Midi { channel, message } => {
                self.last_midi = Instant::now();
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

    fn process_controller(self: &Self, show: &ShowDefinition, channel: u4, controller: u7, value: u7) -> anyhow::Result<Duration> {
        match self.controller_mappings.get(&(channel, controller)) {
            Some(id) => match u8::from(value) {
                127 => self.activate(show, *id),
                0 => self.deactivate(show, *id),
                _ => Ok(DEFAULT_TICK)
            },
            _ => Ok(DEFAULT_TICK)
        }
    }

    fn process_note_on(self: &mut Self, show: &ShowDefinition, channel: u4, key: u7, _velocity: u7) -> anyhow::Result<Duration> {
        match self.note_mappings.get(&(channel, key)) {
            Some(id) => self.activate(show, *id),
            _ => Ok(DEFAULT_TICK)
        }
    }

    fn process_note_off(self: &Self, show: &ShowDefinition, channel: u4, key: u7, _velocity: u7) -> anyhow::Result<Duration> {
        match self.note_mappings.get(&(channel, key)) {
            Some(id) => self.deactivate(&show, *id),
            _ => Ok(DEFAULT_TICK)
        }
    }

    pub fn activate(self: &Self, show: &ShowDefinition, mapping_id: usize) -> anyhow::Result<Duration> {
        let mapping_meta = self.light_mappings.get(&mapping_id).unwrap();
        match &mapping_meta.source.light {
            LightMappingType::Effect(effect) => self.activate_effect(&show, &mapping_meta, &effect),
            LightMappingType::Clip(clip) => self.activate_clip(&show, &mapping_meta, &clip)
        }
    }

    fn activate_effect(self: &Self, show: &ShowDefinition, mapping_meta: &LightMappingMeta, effect: &Effect) -> anyhow::Result<Duration> {
        let mut show_packet = ShowPacket {
            effect: effect.to_effect_id(),
            color: *show.colors.get(&mapping_meta.source.color).unwrap(),
            attack: convert_millis_adr(mapping_meta.source.attack.unwrap_or(0)),
            sustain: convert_millis_sustain(mapping_meta.source.sustain.unwrap_or(0)),
            release: convert_millis_adr(mapping_meta.source.release.unwrap_or(0)),
            param1: 0,
            param2: 0,
            tempo: mapping_meta.source.tempo.unwrap_or(0.0) as u8
        };
        effect.populate_effect_params(&mut show_packet);
        let packet = Packet {
            recipients: &mapping_meta.targets,
            payload: PacketPayload::Show(show_packet),
        };
        self.radio.send(&packet)?;
        // update the receivers triggered by this mapping as active via this mapping
        mapping_meta.receivers.iter().for_each(|r| r.borrow_mut().activate(&mapping_meta.source));
        Ok(DEFAULT_TICK)
    }

    pub fn process_tick(self: &Self) -> anyhow::Result<Duration> {
        //debug!("Tick...");
        // TODO - implement the lights-out packet logic
        Ok(DEFAULT_TICK)
    }

    fn activate_clip(self: &Self, _show: &ShowDefinition, _light_mapping: &LightMappingMeta, _clip: &str) -> anyhow::Result<Duration> {
        Ok(DEFAULT_TICK)
    }

    pub fn deactivate(self: &Self, _show: &ShowDefinition, mapping_id: usize) -> anyhow::Result<Duration>{
        let mapping_meta = self.light_mappings.get(&mapping_id).unwrap();
        match &mapping_meta.source.light {
            LightMappingType::Effect(e) => self.deactivate_effect(mapping_meta, e),
            LightMappingType::Clip(c) => self.deactivate_clip(mapping_meta,c)
        }
    }

    fn deactivate_effect(self: &Self, mapping_meta: &LightMappingMeta, _effect: &Effect) -> anyhow::Result<Duration> {
        
        if !mapping_meta.source.one_shot.unwrap_or(false) {
            let simple_off_path = mapping_meta.receivers.iter().all(
                |r| r.borrow().activated_by(&mapping_meta.source));

            let dynamic_recipients = if simple_off_path {
                None
            } else {
                Some(mapping_meta.receivers.iter()
                    .filter(|r| r.borrow().activated_by(&mapping_meta.source))
                    .map(|r| r.borrow().id)
                    .collect())
            };

            let packet = Packet {
                payload: PacketPayload::Show(ShowPacket::OFF_PACKET),
                recipients: dynamic_recipients.as_ref().unwrap_or(&mapping_meta.targets)
            };
            self.radio.send(&packet)?;
            mapping_meta.receivers.iter().for_each(|r| 
                { r.borrow_mut().deactivate(&mapping_meta.source); });
        }
        Ok(DEFAULT_TICK)
    }

    fn deactivate_clip(self: &Self, _light_mapping: &LightMappingMeta, _clip: &str) -> anyhow::Result<Duration> {
        Ok(DEFAULT_TICK)
    }

    /// a helper function that expands a target list of u8s to a list of receiver state references
    /// (ids representing groups are expanded to references to their underlying receivers)
    fn expand_groups<'c>(group_members: &HashMap<u8,Vec<u8>>, receiver_state: &'c HashMap<u8,Rc<RefCell<ReceiverState>>>, targets: &Vec<u8>) 
    -> Vec<Rc<RefCell<ReceiverState>>> {

        if targets.is_empty() {
            receiver_state.values().map(|rc| rc.clone()).collect()
        } else {
            targets.iter().flat_map(|e|   
                group_members.get(&e)
                    .map_or_else(|| vec![*e].into_iter(), |v| v.clone().into_iter()))
                    .map(|k| receiver_state.get(&k).unwrap().clone())
                    .collect()
        }
    }

    fn create_light_mapping_meta<'c>(m: &'c LightMapping, 
        target_lookup: &HashMap<String,u8>, 
        group_members: &HashMap<u8,Vec<u8>>, 
        receiver_state: &HashMap<u8,Rc<RefCell<ReceiverState>>>) -> Result<LightMappingMeta<'c>> {

        let resolved_targets = match &m.targets {
            None => vec![], // "all receivers" is modeled as an empty target
            Some(tgts) => {
                let mut result: Vec<u8> = vec![];
                for json_tgt in tgts.iter() {
                    let tgt_val = convert_target(json_tgt)?;
                    let otgt = target_lookup.get(&tgt_val);
                    match otgt {
                        Some(id) => result.push(*id),
                        None => return Err(anyhow!("Target in target list does not match any known group or receiver: {}", tgt_val))
                    }
                }
                result
            }
        };
        let resolved_receivers = 
            ShowState::expand_groups(group_members, receiver_state, &resolved_targets);

        Ok(LightMappingMeta {
            source: m,
            targets: resolved_targets,
            receivers: resolved_receivers
        })

    }

}
