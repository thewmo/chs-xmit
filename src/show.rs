use serde::Deserialize;
use std::collections::HashMap;

///
/// This module holds all the structs and functions that
/// model the show JSON and support its deserialization
/// via serde_json
/// 


/// this struct maps directly to the show JSON
#[derive(Debug,Deserialize,Clone)]
pub struct ShowDefinition {
    /// listing of receivers and their groups and LED counts
    pub receivers: Vec<ReceiverConfiguration>,

    /// named colors that can be associated by name with effects and clip effects
    pub colors: HashMap<String,Color>,

    /// associations between MIDI signals and effects or clips
    pub mappings: Vec<LightMapping>,

    /// clip definitions
    pub clips: HashMap<String,Vec<ClipStep>>
}

///
/// effect enum used in JSON. Associated with an EffectId which
/// has as a discriminator the actual u8 that codes for the effect
/// at the receiver level. Struct members code for the effect-specific
/// params that will be sent as param1/param2
/// 
#[derive(Debug,Deserialize,Clone)]
pub enum Effect {
    Pop,
    /// delay quantization controls how many receivers will fire together
    /// multiplier 
    Firecrackers { delay_quantization: u8, delay_multiplier: u8 },
    /// how many leds are illuminated as part of the chase? 
    /// if reverse is true, the chase moves from high number leds to low
    Chase { chase_length: u8, reverse: bool },
    /// division is quarters (1), eights(2) etc relative to tempo
    Strobe { division: u8 }, 
    /// just chase length, reverse is meaningless for the bidi chase effect
    BidiChase { chase_length: u8 },
    /// options mean the same as for regular chase, except for beat_denominator
    /// which divides the tempo to determine how long it takes the head of the
    /// chase to move across the face of one receiver/LED array
    OneShotChase { chase_length: u8, reverse: bool, beat_denominator: u8 },
    BidiOneShotChase { chase_length: u8 },
    /// 1/stride LEDs will be lit, tempo_division is quarters (1), eights(2) etc.
    Sparkle { stride: u8, tempo_division: u8 },
    /// color of the wave goes from the hue (in the color) to alternate_hue
    /// colorspace_fraction is a the fraction of the unit circle (/256) mapped to the array
    Wave { alternate_hue: u8, alternate_brightness: u8, colorspace_phase: u8, colorspace_range: u8 },
    /// flash_decay is how long each triggered flash should take to decay
    /// threshold is how sensitive to be (high values meaning less sensitive to trigger)
    PiezoTrigger { flash_decay: u8, threshold: u8 },
    /// min and max "flame position" in leds illuminated
    Flame { min_flicker: u8, max_flicker: u8 },
    Flame2 { min_flicker: u8, max_flicker: u8 },
    Grass { base_height: u8, blade_top: u8 },
    CircularChase { chase_length: u8, reverse: bool },
    BatteryTest,
    Rainbow { secondary_hue: u8 },
    Twinkle { twinkle_brightness: u8, twinkle_factor: f32 },
    DigitalPin { pin: u8 },
    QueueMovement { steps: u16, rpm: u8, accel: u8, return_to_home: bool },
    Move { steps: u16, rpm: u8, accel: u8, return_to_home: bool },
    SetHome,
}


/// for a given receiver, what is its id, group name, and led count
#[derive(Debug,Deserialize,Clone)]
pub struct ReceiverConfiguration {
    /// the id of the receiver
    pub id: u8,
    /// if a receiver has a name, that name can be used to refer to it in target lists rather than its id
    pub name: Option<String>,
    /// the name of the group the receiver belongs to. note that underlying group ids will be dynamically assigned
    pub group_name: Option<String>,
    /// the number of LEDs in the string
    pub led_count: u16,
    
    pub comment: Option<String>
}

/// the source of a midi mapping whether it be a note or CC (continuous controller)
#[derive(Debug,Deserialize,Clone)]
pub enum MidiMappingType {
    Note { channel: u8, note: String },
    Controller { channel: u8, cc: u8 }
}

/// the target of a mapping, which can be either an effect or a name clip
#[derive(Debug,Deserialize,Clone)]
pub enum LightMappingType {
    Effect(Effect),
    Clip(String)
}

#[derive(Debug,Clone,Copy,Deserialize)]
pub struct Color { pub h: u8, pub s: u8, pub v: u8 }

#[derive(Debug,Deserialize,Clone)]
pub struct LightMapping {
    pub cue: String,
    pub midi: Option<MidiMappingType>,
    pub light: LightMappingType,
    pub color: String,
    pub override_clip_color: Option<bool>,
    pub attack: Option<u32>,
    pub sustain: Option<u32>,
    pub release: Option<u32>,
    pub one_shot: Option<bool>,
    pub tempo: Option<f32>,
    pub modulation: Option<u8>,
    /// targets is optional, if absent, all receivers are targets
    pub targets: Option<Vec<serde_json::Value>>,
}

impl LightMapping {

    pub fn get_id(self: &Self) -> usize {
        self as *const LightMapping as usize
    }
    
}

#[derive(Debug,Deserialize,Clone)]
pub enum ClipStep {
    /// instruction to trigger the contained mapping
    MappingOn(LightMapping),
    /// instruction to trigger "off" the "on" mapping at the specified index
    MappingOff(usize),
    /// wait the specified number of beats
    WaitBeats(f32),
    /// wait the specified number of milliseconds
    WaitMillis(u32),
    /// go back to the clip step at the index
    Loop(usize),
    /// set the current clip-wide color
    SetColor(Color),
    /// set the current clip-wide tempo
    SetTempo(f32),
    /// stop any mappings and terminate the clip
    Stop,
    /// stop another named clip if it's playing
    StopOther(String),
    /// terminate the clip
    End,
}