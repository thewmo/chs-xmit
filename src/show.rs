use serde::Deserialize;
use std::collections::HashMap;

///
/// This module holds all the structs and functions that
/// model the show JSON and support its deserialization
/// via serde_json
/// 


/// this struct maps directly to the show JSON
#[derive(Debug,Deserialize)]
pub struct ShowDefinition {
    /// listing of receivers and their groups and LED counts
    pub receivers: Vec<ReceiverConfiguration>,

    /// named colors that can be associated by name with effects and clip effects
    pub colors: HashMap<String,Color>,

    /// associations between MIDI signals and effects or clips
    pub mappings: Vec<LightMapping>
}

///
/// effect enum used in JSON. Associated with an EffectId which
/// has as a discriminator the actual u8 that codes for the effect
/// at the receiver level. Struct members code for the effect-specific
/// params that will be sent as param1/param2
/// 
#[derive(Debug,Deserialize)]
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
    /// options mean the same as for regular chase
    OneShotChase { chase_length: u8, reverse: bool },
    BidiOneShotChase { chase_length: u8 },
    /// 1/stride LEDs will be lit, tempo_division is quarters (1), eights(2) etc.
    Sparkle { stride: u8, tempo_division: u8 },
    /// color of the wave goes from the hue (in the color) to alternate_hue
    /// colorspace_fraction is a the fraction of the unit circle (/256) mapped to the array
    Wave { alternate_hue: u8, colorspace_fraction: u8 },
    /// flash_decay is how long each triggered flash should take to decay
    /// threshold is how sensitive to be (high values meaning less sensitive to trigger)
    PiezoTrigger { flash_decay: u8, threshold: u8 },
    /// min and max "flame position" in leds illuminated
    Flame { min_flicker: u8, max_flicker: u8 },
    Flame2 { min_flicker: u8, max_flicker: u8 },
    Grass { base_height: u8, blade_top: u8 },
    CircularChase { chase_length: u8, reverse: bool },
    BatteryTest,
    Rainbow { secondary_hue: u8 }
}


/// for a given receiver, what is its id, group name, and led count
#[derive(Debug,Deserialize)]
pub struct ReceiverConfiguration {
    pub id: u8,
    pub group_name: String,
    pub led_count: u16,
}

/// the source of a midi mapping whether it be a note or CC (continuous controller)
#[derive(Debug,Deserialize)]
pub enum MidiMappingType {
    Note { channel: u8, note: String },
    Controller { channel: u8, cc: u8 }
}

/// the target of a mapping, which can be either an effect or a name clip
#[derive(Debug,Deserialize)]
pub enum LightMappingType {
    Effect(Effect),
    Clip(String)
}

#[derive(Debug,Clone,Copy,Deserialize)]
pub struct Color { pub h: u8, pub s: u8, pub v: u8 }

#[derive(Debug,Deserialize)]
pub struct LightMapping {
    pub midi: MidiMappingType,
    pub light: LightMappingType,
    pub color: String,
    pub override_clip_color: bool,
    pub attack: u32,
    pub sustain: u32,
    pub release: u32,
    pub send_note_off: bool,
    pub tempo: u8,
    pub modulation: u8,
    pub targets: Vec<String>
}