use log::{debug,info};
use std::cmp::min;
use std::rc::Rc;
use std::time::{Duration,Instant};
use std::collections::HashMap;
use std::cell::{Cell, RefCell};
use midly::live::LiveEvent;
use midly::MidiMessage;
use midly::num::{u4,u7};
use musical_note::ResolvedNote;
use anyhow::{Result, anyhow};

use crate::config::ConfigFile;
use crate::radio::{Radio,RadioError};
use crate::show::{ClipStep, Color, Effect, LightMapping, LightMappingType, MidiMappingType, ShowDefinition};
use crate::packet::{Command, Packet, PacketPayload, ShowPacket, GROUP_ID_RANGE};
use crate::clip::ClipEngine;

const ALL_RECIPIENTS: Vec<u8> = vec![];

const GLOBAL_OFF_PACKET: Packet = Packet {
    recipients: &ALL_RECIPIENTS,
    payload: PacketPayload::Show(ShowPacket::OFF_PACKET)
};

/// mutable state associated with the show. some things are derived from
/// the show json, other things (eg receiver and clip state) continuously
/// change as the show is performed
/// 
/// lifetimes here - 'a is the director lifetime, 'b is the show lifetime
pub struct ShowState<'a,'b> {

    /// reference to the config
    config: &'a ConfigFile,

    // reference to the radio
    radio: &'a Radio,

    /// the moment of the last effect we triggered
    last_effect: Cell<Instant>,

    /// the last time we sent a timeout-driven "lights out" packet
    last_lights_out: Cell<Instant>,

    /// a map to lookup the u8 ids for named targets
    target_lookup: HashMap<String,u8>,

    /// midi channel/note to light mapping key
    note_mappings: HashMap<(u4,u7), usize>,

    /// midi channel/cc to light mapping key
    controller_mappings: HashMap<(u4,u7), usize>,

    /// quick lookup from light mapping key to the data about that light mapping
    light_mappings: HashMap<usize,LightMappingMeta<'b>>,

    /// a map from receiver id to info about what that receiver should be doing right now
    receiver_state: HashMap<u8,Rc<RefCell<ReceiverState>>>,

    /// a map from a named clip to the play state of that clip
    clip_engine: ClipEngine<'b>
}

pub struct EffectOverrides {
    pub color: Option<Color>,
    pub tempo: Option<f32>,
    pub attack: Option<u32>,
    pub sustain: Option<u32>,
    pub release: Option<u32>
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

    pub fn is_active(self: &Self) -> bool {
        self.trigger_mapping != Self::INACTIVE
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
    pub color: Color,
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

// 'a is the lifetime of the radio (forever)
// 'b is the lifetime of the show definition
impl<'a,'b> ShowState<'a,'b> {
    pub fn new(show: &'b ShowDefinition, radio: &'a Radio, config: &'a ConfigFile) -> Result<ShowState<'a,'b>> {

        let mut target_lookup: HashMap<String,u8> = HashMap::new();
        let mut group_members: HashMap<u8,Vec<u8>> = HashMap::new();
        let mut group_id = GROUP_ID_RANGE.start;
        let mut light_mappings: HashMap<usize, LightMappingMeta> = HashMap::new();
        let mut note_mappings: HashMap<(u4,u7), usize> = HashMap::new();
        let mut controller_mappings: HashMap<(u4,u7), usize> = HashMap::new();
        let mut receiver_state: HashMap<u8,Rc<RefCell<ReceiverState>>> = HashMap::new();

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
                ShowState::create_light_mapping_meta(&show, m, &target_lookup, &group_members, &receiver_state)?);
            match &m.midi {
                Some(MidiMappingType::Note { channel, note }) => {
                    note_mappings.insert(((*channel).into(), ResolvedNote::from_str(&note).unwrap().midi.into()), 
                        m.get_id());
                },
                Some(MidiMappingType::Controller { channel, cc }) => {
                    controller_mappings.insert(((*channel).into(), (*cc).into()), 
                        m.get_id());
                },
                None => {
                    return Err(anyhow!("Non-clip mapping missing a midi mapping element: {:?}", m));
                }
            }
        }

        // preprocess clip-embedded light mappings
        for clip_steps in show.clips.values() {
            for step in clip_steps.iter() {
                match step {
                    ClipStep::MappingOn(m) => {
                        light_mappings.insert(m.get_id(), 
                            ShowState::create_light_mapping_meta(&show, m, &target_lookup, &group_members, &receiver_state)?);
                    },
                    _ => {}
                }
            }
        }

        Ok(ShowState { 
            config,
            radio,
            last_effect: Cell::new(Instant::now()),
            last_lights_out: Cell::new(Instant::now()),
            target_lookup,
            note_mappings, 
            controller_mappings,
            light_mappings,
            receiver_state,
            clip_engine: ClipEngine::new(&show.clips)
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
    
    pub fn process_midi(self: &Self, _ts: u64, buf: Vec<u8>) -> anyhow::Result<()> {
        let midi_event = midly::live::LiveEvent::parse(&buf)?;
        debug!("Received MIDI event: {:?}", midi_event);
        match midi_event {
            LiveEvent::Midi { channel, message } => {
                match message {
                    MidiMessage::Controller { controller, value } => {
                        self.process_controller(channel, controller, value)
                    },
                    MidiMessage::NoteOn { key, vel } => {
                        self.process_note_on(channel, key, vel)
                    },
                    MidiMessage::NoteOff { key, vel } => {
                        self.process_note_off(channel, key, vel)
                    },
                    _ => Ok(())
                }
            },
            _ => Ok(())
        }
    }

    fn process_controller(self: &Self, channel: u4, controller: u7, value: u7) -> anyhow::Result<()> {
        match self.controller_mappings.get(&(channel, controller)) {
            Some(id) => match u8::from(value) {
                127 => self.activate(*id, None),
                0 => self.deactivate(*id),
                _ => Ok(())
            },
            _ => Ok(())
        }
    }

    fn process_note_on(self: &Self, channel: u4, key: u7, _velocity: u7) -> anyhow::Result<()> {
        match self.note_mappings.get(&(channel, key)) {
            Some(id) => self.activate(*id, None),
            _ => Ok(())
        }
    }

    fn process_note_off(self: &Self, channel: u4, key: u7, _velocity: u7) -> anyhow::Result<()> {
        match self.note_mappings.get(&(channel, key)) {
            Some(id) => self.deactivate(*id),
            _ => Ok(())
        }
    }

    pub fn activate(self: &Self, mapping_id: usize, overrides: Option<EffectOverrides>) -> anyhow::Result<()> {
        let mapping_meta = self.light_mappings.get(&mapping_id).unwrap();
        match &mapping_meta.source.light {
            LightMappingType::Effect(effect) => self.activate_effect(&mapping_meta, &effect, overrides),
            LightMappingType::Clip(clip) => self.activate_clip( &mapping_meta, &clip)
        }
    }

    fn activate_effect(self: &Self, mapping_meta: &LightMappingMeta, effect: &Effect, overrides: Option<EffectOverrides>) -> anyhow::Result<()> {
        let mut show_packet = ShowPacket {
            effect: effect.to_effect_id(),
            color: overrides.as_ref().and_then(|o| o.color).unwrap_or(mapping_meta.color),
            attack: convert_millis_adr(overrides.as_ref().and_then(|o| o.attack).or(mapping_meta.source.attack).unwrap_or(0)),
            sustain: convert_millis_sustain(overrides.as_ref().and_then(|o| o.sustain).or(mapping_meta.source.sustain).unwrap_or(0)),
            release: convert_millis_adr(overrides.as_ref().and_then(|o| o.release).or(mapping_meta.source.release).unwrap_or(0)),
            param1: 0,
            param2: 0,
            tempo: overrides.as_ref().and_then(|o| o.tempo).or(mapping_meta.source.tempo).unwrap_or(120.0) as u8
        };
        effect.populate_effect_params(&mut show_packet);
        let packet = Packet {
            recipients: &mapping_meta.targets,
            payload: PacketPayload::Show(show_packet),
        };
        self.radio.send(&packet)?;
        // update the receivers triggered by this mapping as active via this mapping
        mapping_meta.receivers.iter().for_each(|r| r.borrow_mut().activate(&mapping_meta.source));
        self.last_effect.set(Instant::now());
        Ok(())
    }

    /// perform time-based logic - advance playing clips, and implement lights-out logic. called
    /// on every iteration of the show loop, returns the maximum amout of time to wait before
    /// calling tick again.
    pub fn tick(self: &Self) -> anyhow::Result<Duration> {
        let now = Instant::now();

        // advance any clips that are playing
        let play_clips_at = self.clip_engine.play_clips( &self);

        // if no receivers and no clips are active, and it's been n (configurable) seconds since the last midi event,
        // send a lights-out packet once every m (configurable) seconds
        let receiver_active = self.receiver_state.values().any(|rs| rs.borrow().is_active());
        if !receiver_active && !self.clip_engine.is_playing() && 
            self.config.lights_out_window().contains(&(now - self.last_effect.get())) && 
            now - self.last_lights_out.get() >= self.config.lights_out_delay() {

            info!("Sending lights out...");
            self.radio.send(&GLOBAL_OFF_PACKET)?;
            self.last_lights_out.set(now);
        }
        let lights_out_delay = self.config.lights_out_delay();
        Ok(min(lights_out_delay, 
            play_clips_at.map_or(lights_out_delay, |play_clips_at| play_clips_at - now)))
    }

    fn activate_clip(self: &Self, light_mapping: &LightMappingMeta, clip: &str) -> anyhow::Result<()> {
        let override_color = if light_mapping.source.override_clip_color.unwrap_or(false) 
            { Some(light_mapping.color) } else { None };
        self.clip_engine.start_clip(&clip, override_color, light_mapping.source.tempo.unwrap_or(120f32))
    }

    pub fn deactivate(self: &Self, mapping_id: usize) -> anyhow::Result<()>{
        let mapping_meta = self.light_mappings.get(&mapping_id).unwrap();
        match &mapping_meta.source.light {
            LightMappingType::Effect(e) => self.deactivate_effect(mapping_meta, e),
            LightMappingType::Clip(c) => self.deactivate_clip(mapping_meta,c)
        }
    }

    fn deactivate_effect(self: &Self, mapping_meta: &LightMappingMeta, _effect: &Effect) -> anyhow::Result<()> {
        
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
        Ok(())
    }

    fn deactivate_clip(self: &Self, _light_mapping: &LightMappingMeta, clip: &str) -> anyhow::Result<()> {
        self.clip_engine.stop_clip(&clip, &self)
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

    fn create_light_mapping_meta<'c>(show: &ShowDefinition,
        m: &'c LightMapping, 
        target_lookup: &HashMap<String,u8>, 
        group_members: &HashMap<u8,Vec<u8>>, 
        receiver_state: &HashMap<u8,Rc<RefCell<ReceiverState>>>) -> Result<LightMappingMeta<'c>> {

        let resolved_targets = match &m.targets {
            None => ALL_RECIPIENTS, // "all receivers" is modeled as an empty target
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

        let resolved_color = show.colors.get(&m.color)
            .ok_or_else(|| anyhow!("Named color: {} not in color list", m.color))?;

        Ok(LightMappingMeta {
            color: resolved_color.clone(),
            source: m,
            targets: resolved_targets,
            receivers: resolved_receivers
        })

    }

}
