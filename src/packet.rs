use crate::show::Color;
use crate::show::Effect;

///
/// this module concerns itself with building packet buffers from a given
/// mapping
/// 
#[repr(u8)]
#[derive(Debug,Copy,Clone)]
pub enum EffectId {
    Pop = 1,
    Firecrackers = 2,
    Chase = 3,
    Strobe = 4,
    BidiChase = 5,
    OneShotChase = 6,
    BidiOneShotChase = 7,
    Sparkle = 8,
    Wave = 9,
    PiezoTrigger = 10,
    Flame = 11,
    Flame2 = 12,
    Grass = 13,
    CircularChase = 14,
    BatteryTest = 15,
    Rainbow = 16,
}

impl Effect {
    pub fn to_effect_id(self: &Self) -> EffectId {
        match &self {
            Effect::Pop => EffectId::Pop,
            Effect::Firecrackers {..} => EffectId::Firecrackers,
            Effect::Chase {..} => EffectId::Chase,
            Effect::Strobe {..} => EffectId::Strobe,
            Effect::BidiChase {..} => EffectId::BidiChase,
            Effect::OneShotChase {..} => EffectId::OneShotChase,
            Effect::BidiOneShotChase {..} => EffectId::BidiOneShotChase,
            Effect::Sparkle {..} => EffectId::Sparkle,
            Effect::Wave {..} => EffectId::Wave,
            Effect::PiezoTrigger {..} => EffectId::PiezoTrigger,
            Effect::Flame {..} => EffectId::Flame,
            Effect::Flame2 {..} => EffectId::Flame2,
            Effect::Grass {..} => EffectId::Grass,
            Effect::CircularChase {..} => EffectId::CircularChase,
            Effect::BatteryTest => EffectId::BatteryTest,
            Effect::Rainbow {..} => EffectId::Rainbow
        }
    }

    /// 
    /// given a borrow of a vector that is the packet buffer,
    /// translate effect-specific parameters into "current param 1"
    /// and "current param 2" in the radio protocol.
    /// 
    pub fn populate_effect_params(self: &Self, packet: &mut ShowPacket) {
        packet.param1 = 0;
        packet.param2 = 0;
        match &self {
            Effect::Firecrackers { delay_quantization, delay_multiplier} => {
                packet.param1 = *delay_quantization;
                packet.param2 = *delay_multiplier;
            },
            Effect::Chase { chase_length, reverse } => {
                packet.param1 = *chase_length;
                packet.param2 = if *reverse { 1 } else { 0 };
            },
            Effect::Strobe { division } => {
                packet.param1 = *division;
            },
            Effect::BidiChase { chase_length } => {
                packet.param1 = *chase_length;
            },
            Effect::OneShotChase { chase_length, reverse } => {
                packet.param1 = *chase_length;
                packet.param2 = if *reverse { 1 } else { 0 };
            },
            Effect::BidiOneShotChase { chase_length } => {
                packet.param1 = *chase_length;
            },
            Effect::Sparkle { stride, tempo_division } => {
                packet.param1 = *stride;
                packet.param2 = *tempo_division;
            },
            Effect::Wave { alternate_hue, colorspace_fraction } => {
                packet.param1 = *alternate_hue;
                packet.param2 = *colorspace_fraction;
            },
            Effect::PiezoTrigger { flash_decay, threshold } => {
                packet.param1 = *flash_decay;
                packet.param2 = *threshold;
            },
            Effect::Flame { min_flicker, max_flicker} => {
                packet.param1 = *min_flicker;
                packet.param2 = *max_flicker;
            },
            Effect::Flame2 { min_flicker, max_flicker } => {
                packet.param1 = *min_flicker;
                packet.param2 = *max_flicker;
            },
            Effect::Grass { base_height, blade_top } => {
                packet.param1 = *base_height;
                packet.param2 = *blade_top;
            },
            Effect::CircularChase { chase_length, reverse } => {
                packet.param1 = *chase_length;
                packet.param2 = if *reverse { 1 } else { 0 };
            }
            _ => {}
        }
    }
}

#[derive(Debug,Copy,Clone)]
pub enum Command {
    SetGroup { group_id: u8 },
    SetLedCount { led_count: u16 },
    NewBrightness { brightness: u8 },
    NewTempo { tempo: u8 },
    Reset
}

impl Command {
    pub fn to_id(self: &Self) -> CommandId {
        match self {
            Command::SetGroup {..} => CommandId::SetGroup,
            Command::SetLedCount {..} => CommandId::SetLedCount,
            Command::NewBrightness {..} => CommandId::NewBrightness,
            Command::NewTempo {..} => CommandId::NewTempo,
            Command::Reset => CommandId::Reset
        }
    }

    pub fn marshal(self: &Self, buf: &mut Vec<u8>) {
        buf.push(self.to_id() as u8);
        self.populate_params(buf);
    }

    pub fn populate_params(self: &Self, buf: &mut Vec<u8>) {
        match self {
            Command::SetGroup { group_id} => {
                buf.push(*group_id);
                buf.push(0);
                buf.push(0);
            },
            Command::SetLedCount { led_count } => {
                buf.push((led_count >> 8) as u8);
                buf.push((led_count & 0xFF) as u8);
                buf.push(0);
            },
            Command::NewBrightness { brightness } => {
                buf.push(*brightness);
                buf.push(0);
                buf.push(0);
            },
            Command::NewTempo { tempo } => {
                buf.push(*tempo);
                buf.push(0);
                buf.push(0);
            },
            Command::Reset => {
                buf.extend_from_slice(&[0;3]);
            }
        }
    }
}

#[repr(u8)]
#[derive(Debug,Copy,Clone)]
pub enum CommandId {
    SetGroup = 109,
    SetLedCount = 110,
    NewBrightness = 127,
    NewTempo = 128,
    Reset = 255
}

#[derive(Debug)]
pub struct Packet {
    pub recipients: Vec<u8>,
    pub payload: PacketPayload
}

#[derive(Debug,Copy,Clone)]
pub enum PacketPayload {
    Control(Command),
    Show(ShowPacket)
}

impl Packet {
    pub fn marshal(self: &Self, from_id: u8, packet_id: u8, flags: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.push(0); // we'll poke the length in here later
        // recipient address is next, this is either 255 for broadcast/multi *or* a single receiver id
        buf.push(match self.recipients.len() {
            1 => self.recipients[0],
            _ => 0xFF,
        });
        // three bytes that are here for compatibility with RadioHead
        buf.push(from_id);
        buf.push(packet_id);
        buf.push(flags);
        match &self.payload {
            PacketPayload::Control(p) => p.marshal(&mut buf),
            PacketPayload::Show(p) => p.marshal(&mut buf),
        }
        if self.recipients.len() > 1 {
            for r in self.recipients.iter() {
                buf.push(*r)
            }
        }
        // update the head with the size
        buf[0] = (buf.len() - 1) as u8;
        buf
    }
}

#[derive(Debug,Copy,Clone)]
pub struct ShowPacket {
    // the effect to perform
    pub effect: EffectId,

    // the color (will be sent as three bytes, hsv)
    pub color: Color,
    
    // the duration of the "attack"/fade-in of the effect, in 10s of millis
    pub attack: u8,

    // the maximum duration of the steady-state effect, or 255 to continue until an "effect stop" is received
    pub sustain: u8,
    
    // the duration of the "release"/fade-out of the effect, in 10s of millis
    pub release: u8,

    // an arbitrary effect-specific parameter    
    pub param1: u8,

    // another arbitrary effect-specific parameter    
    pub param2: u8,

    // if the effect has a recurring motion element, that effect should repeat this many times per minute
    pub tempo: u8,
}

impl ShowPacket {
    pub fn marshal(self: &Self, buf: &mut Vec<u8>) {
        buf.push(self.effect as u8);
        buf.push(self.color.h);
        buf.push(self.color.s);
        buf.push(self.color.v);
        buf.push(self.attack);
        buf.push(self.sustain);
        buf.push(self.release);
        buf.push(self.param1);
        buf.push(self.param2);
        buf.push(self.tempo);
    }
}