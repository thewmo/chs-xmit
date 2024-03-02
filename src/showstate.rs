use log::{debug,info};
use std::cmp::min;
use std::rc::Rc;
use std::time::{Duration,Instant};
use std::collections::{HashMap};
use std::cell::RefCell;
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

const SUSTAIN_CONTROLLER: u8 = 64;
const TEST_CONTROLLER : u8 = 102;

const ALL_RECIPIENTS: Vec<u8> = vec![];

const GLOBAL_RESET_PACKET: Packet = Packet {
    recipients: &ALL_RECIPIENTS,
    payload: PacketPayload::Control(Command::Reset)
};

const GLOBAL_OFF_PACKET: Packet = Packet {
    recipients: &ALL_RECIPIENTS,
    payload: PacketPayload::Show(ShowPacket::OFF_PACKET)
};

const GLOBAL_TEST_PACKET: Packet = Packet {
    recipients: &ALL_RECIPIENTS,
    payload: PacketPayload::Show(ShowPacket::TEST_PACKET)
};

/// immutable state associated with the show. some things are derived from
/// the show json, other things (eg receiver and clip state) continuously
/// change as the show is performed
/// 
/// lifetimes here - 'a is the director lifetime, 'b is the show lifetime
pub struct ShowState<'a,'b> {

    /// reference to the config
    config: &'a ConfigFile,

    // reference to the radio
    radio: &'a Radio,

    /// the show definition
    show: &'b ShowDefinition,

    /// a map from group id to the groups members
    group_members: HashMap<u8,Vec<u8>>,

    /// a map to lookup the u8 ids for named targets
    target_lookup: HashMap<String,u8>,

    /// midi channel/note to light mapping key
    note_mappings: HashMap<(u4,u7), Vec<usize>>,

    /// midi channel/cc to light mapping key
    controller_mappings: HashMap<(u4,u7), Vec<usize>>,
    
    /// a map from a named clip to the play state of that clip
    /// note that the clip engine uses interior mutability so we can treat it as immutable
    clip_engine: ClipEngine<'b>,
}

/// mutable state associated with the show (receiver and clip state)
/// as well as things with references to the immutable show state. 
/// (including those things in the immutable show state would create 
/// a self-referential object of which the borrow checker is not fond)
pub struct MutableShowState<'a> {
    
    /// the moment of the last effect we triggered
    last_effect: Instant,

    /// the last time we sent a timeout-driven "lights out" packet
    last_lights_out: Instant,
    
    /// quick lookup from light mapping key to the data about that light mapping
    light_mappings: HashMap<usize,LightMappingMeta<'a>>,

    /// a map from receiver id to info about what that receiver should be doing right now
    receiver_state: HashMap<u8,Rc<RefCell<ReceiverState>>>,

    /// are we currently buffering effect-off messages
    sustain: bool,

    /// a buffer of pending effect ids that should be disabled 
    pending_off: Vec<usize>
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
        let mut note_mappings: HashMap<(u4,u7), Vec<usize>> = HashMap::new();
        let mut controller_mappings: HashMap<(u4,u7), Vec<usize>> = HashMap::new();

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
        }
        
        // build maps from midi triggers to mappings
        for m in show.mappings.iter() {
            match &m.midi {
                Some(MidiMappingType::Note { channel, note }) => {
                    note_mappings.entry(((*channel).into(), ResolvedNote::from_str(&note).unwrap().midi.into()))
                    .or_insert_with(Vec::new).push(m.get_id());
                },
                Some(MidiMappingType::Controller { channel, cc }) => {
                    controller_mappings.entry(((*channel).into(), (*cc).into()))
                    .or_insert_with(Vec::new).push(m.get_id());
                },
                None => {
                    return Err(anyhow!("Non-clip mapping missing a midi mapping element: {:?}", m));
                }
            }
        }

        Ok(ShowState { 
            config,
            radio,
            show,
            group_members,
            target_lookup,
            note_mappings, 
            controller_mappings,
            clip_engine: ClipEngine::new(&show.clips)
     })
    }
    
    pub fn create_mutable_state(self: &Self) -> anyhow::Result<MutableShowState> {
        let mut receiver_state: HashMap<u8,Rc<RefCell<ReceiverState>>> = HashMap::new();
        let mut light_mappings: HashMap<usize, LightMappingMeta> = HashMap::new();

        for r in self.show.receivers.iter() {
            receiver_state.insert(r.id, Rc::new(RefCell::new(ReceiverState::new(r.id))));
        }

        // preprocess light mappings
        for m in self.show.mappings.iter() {
            light_mappings.insert(m.get_id(), self.create_light_mapping_meta( m, &receiver_state)?);
        }
        
        // preprocess clip-embedded light mappings
        for clip_steps in self.show.clips.values() {
            for step in clip_steps.iter() {
                match step {
                    ClipStep::MappingOn(m) => {
                        light_mappings.insert(m.get_id(), self.create_light_mapping_meta(m, &receiver_state)?);
                    },
                    _ => {}
                }
            }
        }

        Ok(MutableShowState {
            last_effect: Instant::now(),
            last_lights_out: Instant::now(),
            light_mappings,
            receiver_state,
            sustain: false,
            pending_off: Vec::<usize>::new()
        })
    }

    fn create_light_mapping_meta<'c>(self: &Self,
        m: &'c LightMapping, 
        receiver_state: &HashMap<u8,Rc<RefCell<ReceiverState>>>) -> Result<LightMappingMeta<'c>> {

        let resolved_targets = match &m.targets {
            None => ALL_RECIPIENTS, 
            Some(tgts) => {
                let mut result: Vec<u8> = vec![];
                for json_tgt in tgts.iter() {
                    let tgt_val = convert_target(json_tgt)?;
                    let otgt = self.target_lookup.get(&tgt_val);
                    match otgt {
                        Some(id) => result.push(*id),
                        None => return Err(anyhow!("Target in target list does not match any known group or receiver: {}", tgt_val))
                    }
                }
                result
            }
        };
        let resolved_receivers = self.expand_groups(receiver_state, &resolved_targets);

        let resolved_color = self.show.colors.get(&m.color)
            .ok_or_else(|| anyhow!("Named color: {} not in color map", m.color))?;

        Ok(LightMappingMeta {
            color: resolved_color.clone(),
            source: m,
            targets: resolved_targets,
            receivers: resolved_receivers
        })

    }
    
    /// a helper function that expands a target list of u8s to a list of receiver state references
    /// (ids representing groups are expanded to references to their underlying receivers)
    fn expand_groups<'c>(self: &Self, receiver_state: &'c HashMap<u8,Rc<RefCell<ReceiverState>>>, targets: &Vec<u8>) 
    -> Vec<Rc<RefCell<ReceiverState>>> {

        if targets.is_empty() {
            receiver_state.values().map(|rc| rc.clone()).collect()
        } else {
            targets.iter().flat_map(|e|   
                self.group_members.get(&e)
                    .map_or_else(|| vec![*e].into_iter(), |v| v.clone().into_iter()))
                    .map(|k| receiver_state.get(&k).unwrap().clone())
                    .collect()
        }
    }

    /// Send control packets to all the receivers telling them
    /// what group they're in and how many leds they have
    pub fn configure_receivers(self: &Self) -> Result<(), RadioError> {
        // reset everybody because receiving a 
        self.radio.send(&GLOBAL_RESET_PACKET)?;
        for receiver in self.show.receivers.iter() {

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
    
    pub fn process_midi(self: &Self, midi_event: &LiveEvent, state: &mut MutableShowState) -> anyhow::Result<()> {
        debug!("Received MIDI event: {:?}", midi_event);
        match midi_event {
            LiveEvent::Midi { channel, message } => {
                match message {
                    MidiMessage::Controller { controller, value } => {
                        self.process_controller(*channel, *controller, *value, state)
                    },
                    MidiMessage::NoteOn { key, vel } => {
                        self.process_note_on(*channel, *key, *vel, state)
                    },
                    MidiMessage::NoteOff { key, vel } => {
                        self.process_note_off(*channel, *key, *vel, state)
                    },
                    _ => Ok(())
                }
            },
            _ => Ok(())
        }
    }

    fn process_special_controllers(self: &Self, channel: u4, controller: u7, value: u7, state: &mut MutableShowState) -> anyhow::Result<bool> {
        if channel == self.config.midi_control_channel {
            match controller.into() {
                SUSTAIN_CONTROLLER => {
                    if value == 127 {
                        info!("sustain activated, will buffer midi deactivations");
                        state.sustain = true;
                    } else if value == 0 {
                        info!("sustain released, performing buffered deactivations");
                        state.sustain = false;
                        // clone to appease the borrow checker
                        for e in state.pending_off.clone().iter() {
                            self.deactivate(*e, state)?;
                        }
                        state.pending_off.clear();
                    }
                    Ok(true)
                },
                TEST_CONTROLLER => {
                    if value == 127 {
                        info!("midi test received, firing test packet");
                        self.radio.send(&GLOBAL_TEST_PACKET)?;
                        state.last_effect = Instant::now();
                    } else {
                        self.radio.send(&GLOBAL_OFF_PACKET)?;
                    }
                    Ok(true)
                },
                _ => Ok(false)
            }
        } else {
            Ok(false)
        }
    }

    fn process_controller(self: &Self, channel: u4, controller: u7, value: u7, state: &mut MutableShowState) -> anyhow::Result<()> {
        if self.process_special_controllers( channel, controller, value, state)? {
            return Ok(())
        }
        match self.controller_mappings.get(&(channel, controller)) {
            Some(ids) => {
                for id in ids {
                    match u8::from(value) {
                        127 => self.activate(*id, None, state)?,
                        0 => self.deactivate_from_midi(*id, state)?,
                        _ => ()
                    }
                }
                Ok(())
            },
            _ => Ok(())
        }
    }

    fn process_note_on(self: &Self, channel: u4, key: u7, _velocity: u7, state: &mut MutableShowState) -> anyhow::Result<()> {
        match self.note_mappings.get(&(channel, key)) {
            Some(ids) => {
                for id in ids {
                    self.activate(*id, None, state)?;
                }
                Ok(())
            },
            _ => Ok(())
        }
    }

    fn process_note_off(self: &Self, channel: u4, key: u7, _velocity: u7, state: &mut MutableShowState) -> anyhow::Result<()> {
        match self.note_mappings.get(&(channel, key)) {
            Some(ids) => {
                for id in ids {
                    self.deactivate_from_midi(*id, state)?;
                }
                Ok(())
            },
            _ => Ok(())
        }
    }

    pub fn activate(self: &Self, mapping_id: usize, overrides: Option<EffectOverrides>, state: &mut MutableShowState) -> anyhow::Result<()> {        
        let light = &state.light_mappings.get(&mapping_id).unwrap().source.light;
        match light {
            LightMappingType::Effect(effect) => self.activate_effect(mapping_id, &effect, overrides, state),
            LightMappingType::Clip(clip) => self.activate_clip( mapping_id, &clip, state)
        }
    }

    fn activate_effect(self: &Self, mapping_id: usize, effect: &Effect, overrides: Option<EffectOverrides>, state: &mut MutableShowState) -> anyhow::Result<()> {
        let mapping_meta = state.light_mappings.get(&mapping_id).unwrap();
        info!("activate cue: {}", mapping_meta.source.cue);

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
        state.last_effect = Instant::now();
        Ok(())
    }

    /// perform time-based logic - advance playing clips, and implement lights-out logic. called
    /// on every iteration of the show loop, returns the maximum amout of time to wait before
    /// calling tick again.
    pub fn tick(self: &Self, state: &mut MutableShowState) -> anyhow::Result<Duration> {
        let now = Instant::now();

        // advance any clips that are playing
        let play_clips_at = self.clip_engine.play_clips( &self, state);

        // if no receivers and no clips are active, and it's been n (configurable) seconds since the last midi event,
        // send a lights-out packet once every m (configurable) seconds
        let receiver_active = state.receiver_state.values().any(|rs| rs.borrow().is_active());
        if !receiver_active && !self.clip_engine.is_playing() && 
            self.config.lights_out_window().contains(&(now - state.last_effect)) && 
            now - state.last_lights_out >= self.config.lights_out_delay() {

            debug!("lights out");
            self.radio.send(&GLOBAL_OFF_PACKET)?;
            state.last_lights_out = now;
        }
        let lights_out_delay = self.config.lights_out_delay();
        Ok(min(lights_out_delay, 
            play_clips_at.map_or(lights_out_delay, |play_clips_at| play_clips_at - now)))
    }

    fn activate_clip(self: &Self, mapping_id: usize, clip: &str, state: &mut MutableShowState) -> anyhow::Result<()> {
        let light_mapping = state.light_mappings.get(&mapping_id).unwrap();
        let override_color = if light_mapping.source.override_clip_color.unwrap_or(false) 
            { Some(light_mapping.color) } else { None };
        self.clip_engine.start_clip(&clip, override_color, light_mapping.source.tempo.unwrap_or(120f32))
    }

    /// a wrapper around deactivate calls coming from a live source,
    /// as such calls need to be buffered if we're in "sustain" mode
    fn deactivate_from_midi(self: &Self, mapping_id: usize, state: &mut MutableShowState) -> anyhow::Result<()> {
        if state.sustain {
            state.pending_off.push(mapping_id);
            Ok(())
        } else {
            self.deactivate(mapping_id, state)
        }
    }

    pub fn deactivate(self: &Self, mapping_id: usize, state: &mut MutableShowState) -> anyhow::Result<()>{
        let mapping_meta = state.light_mappings.get(&mapping_id).unwrap();
        if !mapping_meta.source.one_shot.unwrap_or(false) {
            match &mapping_meta.source.light {
                LightMappingType::Effect(e) => self.deactivate_effect(mapping_meta, e),
                LightMappingType::Clip(c) => self.clip_engine.stop_clip(&c, &self, state)
            }
        } else {
            Ok(())
        }
    }

    fn deactivate_effect(self: &Self, mapping_meta: &LightMappingMeta, _effect: &Effect) -> anyhow::Result<()> {
        info!("deactivate cue: {}",  mapping_meta.source.cue);

        // we can take the simple path if all receivers activated by this effect are still
        // activated by this effect
        let simple_off_path = mapping_meta.receivers.iter().all(
            |r| r.borrow().activated_by(&mapping_meta.source));

        let dynamic_recipients = if simple_off_path {
            None
        } else {
            // otherwise we have to calculate receivers to deactivate individually by finding ones
            // this effect activated
            Some(mapping_meta.receivers.iter()
                .filter(|r| r.borrow().activated_by(&mapping_meta.source))
                .map(|r| r.borrow().id)
                .collect())
        };

        let packet = Packet {
            payload: PacketPayload::Show(ShowPacket::OFF_PACKET),
            recipients: dynamic_recipients.as_ref().unwrap_or(&mapping_meta.targets)
        };
        debug!("deactivate recipients list computed to be: {:#?}", packet.recipients);

        // want to skip sending anything if we had to dynamically compute the off list and it came up empty
        // (all receivers were captured by another effect, so there's nothing to do)
        if dynamic_recipients.is_none() || dynamic_recipients.as_ref().is_some_and(|r| !r.is_empty()) {
            self.radio.send(&packet)?;
            // update each receiver state as deactivated
            for receiver in &mapping_meta.receivers {
                receiver.borrow_mut().deactivate(&mapping_meta.source);
            }
        }
        Ok(())
    }
    
}
