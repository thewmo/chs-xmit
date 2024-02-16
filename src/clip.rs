use std::{collections::{HashMap, HashSet}, time::{Duration, Instant}};
use log::error;
use crate::{show::{ClipStep, LightMapping, ShowDefinition}, showstate::ShowState};

pub struct ClipEngine {
    clip_state: HashMap<String,ClipState>
}

impl ClipEngine {
    pub fn new(def: &HashMap<String,Vec<ClipStep>>) -> ClipEngine {
        let mut state: HashMap<String,ClipState> = HashMap::new();
        for clip in def.keys() {
            state.insert(clip.clone(), ClipState::new());
        }
        ClipEngine { clip_state: state }
    }

    pub fn start_clip(self: &mut Self, 
        clip_name: &str,
        show_def: &ShowDefinition, 
        show_state: &ShowState, 
        trigger: &LightMapping) -> Option<Instant> {

        self.clip_state.get_mut(clip_name).unwrap().start(show_def, 
            show_def.clips.get(clip_name).unwrap(), 
            show_state, trigger)
    }

    pub fn stop_clip(self: &mut Self, 
        clip_name: &str,
        show_def: &ShowDefinition, 
        show_state: &ShowState) {

        self.clip_state.get_mut(clip_name).unwrap().stop(show_def, show_state);
    }

    pub fn play_clips(self: &mut Self, 
        show_def: &ShowDefinition, 
        show_state: &ShowState) -> Option<Instant> {

        let mut play_again: Option<Instant> = None;
        for (clip_name, state) in self.clip_state.iter_mut() {
            let clip_def = show_def.clips.get(clip_name).unwrap();
            let play_this_again = state.play(show_def, clip_def, show_state);

            if play_this_again.is_some() && (play_again.is_none() || play_this_again.unwrap() < play_again.unwrap()) {
                play_again = play_this_again;
            }
        }
        play_again
    }
}

pub struct ClipState {
    playing: bool,
    step: usize,
    advance_at: Instant,
    tempo: Option<f32>,
    override_color: Option<String>,
    active_mappings: HashSet<usize>
}

impl ClipState {

    fn beats_to_millis(self: &Self, beats: f32) -> u64 {
        ((beats * 60000f32)/self.tempo.unwrap()) as u64
    }

    pub fn new() -> ClipState {
        ClipState {
            playing: false,
            step: 0,
            advance_at: Instant::now(),
            tempo: None,
            override_color: None,
            active_mappings: HashSet::new()
        }
    }

    pub fn start(self: &mut Self, 
        show_def: &ShowDefinition, 
        clip_def: &Vec<ClipStep>, 
        show_state: &ShowState, 
        trigger: &LightMapping) -> Option<Instant> {
        self.playing = true;
        self.step = 0;
        self.advance_at = Instant::now();
        self.tempo = trigger.tempo;
        self.override_color = if trigger.override_clip_color.unwrap_or(false) { Some(trigger.color.clone()) } else { None };
        self.play(show_def, clip_def, show_state)
    }

    pub fn play(self: &mut Self, show_def: &ShowDefinition, clip_def: &Vec<ClipStep>, show_state: &ShowState) -> Option<Instant> {
        let now = Instant::now();
        while self.playing {
            if self.advance_at > now {
                return Some(self.advance_at)
            }
            match &clip_def[self.step] {
                ClipStep::MappingOn(mapping) => {
                    show_state.activate(show_def, mapping.get_id());
                    if mapping.one_shot.unwrap_or(false) {
                        self.active_mappings.insert(self.step);
                    }
                    self.step = self.step + 1;

                },
                ClipStep::MappingOff(index) => {
                    if let ClipStep::MappingOn(mapping) = &clip_def[*index] {
                        show_state.deactivate(show_def, mapping.get_id());
                        self.active_mappings.remove(index);
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
                    self.tempo = Some(*tempo);
                    self.step = self.step + 1;
                },
                ClipStep::Stop => {
                    self.stop(show_def, show_state);
                },
                ClipStep::StopOther(name) => {
                    self.step = self.step + 1;
                    // TODO pass index on to clip engine
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

    pub fn stop(self: &mut Self, show_def: &ShowDefinition, show_state: &ShowState) {
        for id in self.active_mappings.drain() {
            show_state.deactivate(show_def, id);
        }
        self.playing = false;
        self.step = 0;
    }

}