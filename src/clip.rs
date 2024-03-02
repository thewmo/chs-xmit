use std::{cell::RefCell, collections::{HashMap, HashSet}, time::{Duration, Instant}};
use log::{info,error};
use crate::{show::{ClipStep, Color}, showstate::{EffectOverrides, MutableShowState, ShowState}};

pub struct ClipEngine<'a> {
    clip_state: HashMap<String, RefCell<ClipState<'a>>>
}

impl <'a> ClipEngine<'a> {
    pub fn new(def: &'a HashMap<String,Vec<ClipStep>>) -> ClipEngine<'a> {
        let mut state: HashMap<String,RefCell<ClipState>> = HashMap::new();
        for clip in def.keys() {
            state.insert(clip.clone(), RefCell::new(ClipState::new(def.get(clip).unwrap())));
        }
        ClipEngine { clip_state: state }
    }

    pub fn start_clip(self: &Self, clip_name: &str, override_color: Option<Color>, tempo: f32) -> anyhow::Result<()> {
        info!("Starting clip: {}", clip_name);
        self.clip_state.get(clip_name).unwrap().borrow_mut().start(override_color, tempo)
    }

    pub fn stop_clip(self: &Self, clip_name: &str, show_state: &ShowState, mut_state: &mut MutableShowState) -> anyhow::Result<()> {
        info!("Stopping clip: {}", clip_name);
        self.clip_state.get(clip_name).unwrap().borrow_mut().stop(show_state, mut_state)
    }

    pub fn play_clips(self: &Self, show_state: &ShowState, mut_state: &mut MutableShowState) -> Option<Instant> {

        let mut play_again_at: Option<Instant> = None;
        for (_clip_name, state) in self.clip_state.iter() {
            let play_this_again_at = state.borrow_mut().play(show_state, self, mut_state);
            if play_this_again_at.is_some() && (play_again_at.is_none() || play_this_again_at.unwrap() < play_again_at.unwrap()) {
                play_again_at = play_this_again_at;
            }
        }
        play_again_at
    }

    pub fn is_playing(self: &Self) -> bool {
        self.clip_state.values().any(|cs| cs.borrow().is_playing())
    }

}

pub struct ClipState<'a> {
    playing: bool,
    step: usize,
    advance_at: Instant,
    tempo: f32,
    override_color: Option<Color>,
    active_mappings: HashSet<usize>,
    steps: &'a Vec<ClipStep>
}

impl <'a> ClipState<'a> {

    fn beats_to_millis(self: &Self, beats: f32) -> u64 {
        ((beats * 60000f32)/self.tempo) as u64
    }

    pub fn new(steps: &'a Vec<ClipStep>) -> ClipState<'a> {
        ClipState {
            playing: false,
            step: 0,
            advance_at: Instant::now(),
            tempo: 120f32,
            override_color: None,
            active_mappings: HashSet::new(),
            steps
        }
    }

    pub fn start(self: &mut Self, override_color: Option<Color>, tempo: f32) -> anyhow::Result<()> {
        self.playing = true;
        self.step = 0;
        self.advance_at = Instant::now();
        self.tempo = tempo;
        self.override_color = override_color;
        Ok(())
    }

    pub fn play(self: &mut Self, show_state: &ShowState, engine: &ClipEngine, mut_state: &mut MutableShowState) -> Option<Instant> {
        let now = Instant::now();
        while self.playing && self.step < self.steps.len() {
            if self.advance_at > now {
                return Some(self.advance_at)
            }
            match &self.steps[self.step] {
                ClipStep::MappingOn(mapping) => {
                    let overrides = Some(EffectOverrides {
                        color: self.override_color,
                        tempo: Some(self.tempo),
                        attack: None,
                        sustain: None,
                        release: None
                    });
                    let _ = show_state.activate(mapping.get_id(), overrides, mut_state);
                    if !mapping.one_shot.unwrap_or(false) {
                        self.active_mappings.insert(mapping.get_id());
                    }
                    self.step = self.step + 1;

                },
                ClipStep::MappingOff(index) => {
                    if let ClipStep::MappingOn(mapping) = &self.steps[*index] {
                        let _ = show_state.deactivate(mapping.get_id(), mut_state);
                        self.active_mappings.remove(&mapping.get_id());
                    } else {
                        error!("Mapping off step at index: {} does not point to mapping on step with index: {}", self.step, *index);
                    }
                    self.step = self.step + 1;
                },
                ClipStep::End => {
                    self.playing = false;
                },
                ClipStep::Loop(index) => { 
                    self.step = *index 
                },
                ClipStep::SetColor(color) => {
                    self.override_color = Some(color.clone());
                    self.step = self.step + 1;
                },
                ClipStep::SetTempo(tempo) => {
                    self.tempo = *tempo;
                    self.step = self.step + 1;
                },
                ClipStep::Stop => {
                    let _ = self.stop(show_state, mut_state);
                },
                ClipStep::StopOther(name) => {
                    let _ = engine.stop_clip(name, show_state, mut_state);
                    self.step = self.step + 1;
                },
                ClipStep::WaitBeats(beats) => {
                    self.advance_at = now + Duration::from_millis(self.beats_to_millis(*beats));
                    self.step = self.step + 1;
                },
                ClipStep::WaitMillis(millis) => {
                    self.advance_at = now + Duration::from_millis(*millis as u64);
                    self.step = self.step + 1;
                }
            }
        }
        None
    }

    pub fn stop(self: &mut Self, show_state: &ShowState, mut_state: &mut MutableShowState) -> anyhow::Result<()> {
        for id in self.active_mappings.drain() {
            show_state.deactivate(id, mut_state)?;
        }
        self.playing = false;
        self.step = 0;
        Ok(())
    }

    pub fn is_playing(self: &Self) -> bool {
        self.playing
    }

}