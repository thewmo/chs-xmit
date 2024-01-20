use serde::Deserialize;
use crate::packet::EffectId;

/// this struct maps directly to the show JSON
#[derive(Debug,Deserialize)]
pub struct Show {
    pub receivers: Vec<ReceiverConfiguration>,
    pub mappings: Vec<LightMapping>
}

///
/// This module holds all the structs and functions that
/// model the show JSON and support its deserialization
/// via serde_json
/// 
#[derive(Debug,Deserialize)]
pub enum Effect {
    Pop,
    Firecrackers { delay_quantization: u8, delay_multiplier: u8 },
    Chase,
    Strobe,
    BidiChase,
    OneShotChase,
    BidiOneShotChase,
    Sparkle,
    Wave,
    PiezoTrigger,
    Flame,
    Flame2,
    Grass,
    CircularChase,
    BatteryTest,
    Rainbow { secondary_hue: u8 }
}

impl Effect {
    pub fn to_effect_id(self: &Self) -> EffectId {
        match &self {
            Effect::Pop => EffectId::Pop,
            Effect::Firecrackers {..} => EffectId::Firecrackers,
            Effect::Chase => EffectId::Chase,
            Effect::Strobe => EffectId::Strobe,
            Effect::BidiChase => EffectId::BidiChase,
            Effect::OneShotChase => EffectId::OneShotChase,
            Effect::BidiOneShotChase => EffectId::BidiOneShotChase,
            Effect::Sparkle => EffectId::Sparkle,
            Effect::Wave => EffectId::Wave,
            Effect::PiezoTrigger => EffectId::PiezoTrigger,
            Effect::Flame => EffectId::Flame,
            Effect::Flame2 => EffectId::Flame2,
            Effect::Grass => EffectId::Grass,
            Effect::CircularChase => EffectId::CircularChase,
            Effect::BatteryTest => EffectId::BatteryTest,
            Effect::Rainbow {..} => EffectId::Rainbow
        }
    }

    /// 
    /// given a borrow of a vector that is the packet buffer,
    /// translate effect-specific parameters into "current param 1"
    /// and "current param 2" in the radio protocol.
    /// 
    fn populate_packet_params(self: &Self, buf: &mut Vec<u8>) {
        match &self {
            Effect::Firecrackers { delay_quantization: q, delay_multiplier: m} => {
                buf.push(*q);
                buf.push(*m);
            },
            _ => { 
                buf.push(0); 
                buf.push(0);
            }
        }
    }
}

#[derive(Debug,Deserialize)]
pub struct ReceiverConfiguration {
    pub id: u8,
    pub group_name: String,
    pub led_count: u16,
}

#[derive(Debug,Deserialize)]
pub enum MidiMappingType {
    Note { channel: u8, note: String },
    Controller { channel: u8, cc: u8 }
}

#[derive(Debug,Deserialize)]
pub enum LightMappingType {
    Effect(Effect),
    Clip(String)
}

#[derive(Debug,Clone,Copy,Deserialize)]
pub struct HSV(pub u8, pub u8, pub u8);

#[derive(Debug,Deserialize)]
pub struct LightMapping {
    pub midi: MidiMappingType,
    pub light: LightMappingType,
    pub color: HSV,
    pub override_clip_color: bool,
    pub attack: u32,
    pub sustain: u32,
    pub release: u32,
    pub send_note_off: bool,
    pub tempo: u8,
    pub modulation: u8,
    pub targets: Vec<String>
}