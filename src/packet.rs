use crate::types::Color;

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

#[repr(u8)]
#[derive(Debug,Copy,Clone)]
pub enum Command {
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
    Control(ControlPacket),
    Show(ShowPacket)
}

#[derive(Debug,Copy,Clone)]
pub struct ControlPacket {
    pub command_id: Command,
    pub param1: u8,
    pub param2: u8,
    pub request_reply: bool,
}

impl ControlPacket {
    fn marshal(self: &Self, buf: &mut Vec<u8>) {
        buf.push(0xFFu8); // "effect" value that tells the receiver this is a command
        buf.push(self.command_id as u8);
        buf.push(self.param1);
        buf.push(self.param2);
        buf.push(if self.request_reply { 1 } else { 0 })
    }
}

impl Packet {
    pub fn marshal(self: &Self, from_id: u8, packet_id: u8, flags: u8) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.push(0); // we'll poke the length in here later
        // recipient address is next, this is either 255 for broadcast or a single receiver id
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
        buf.push(self.color.hue);
        buf.push(self.color.saturation);
        buf.push(self.color.brightness);
        buf.push(self.attack);
        buf.push(self.sustain);
        buf.push(self.release);
        buf.push(self.param1);
        buf.push(self.param2);
        buf.push(self.tempo);
    }
}