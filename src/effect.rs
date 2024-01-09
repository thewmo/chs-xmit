#[repr(u8)]
#[derive(Debug,Copy,Clone)]
pub enum Effect {
    POP = 1,
    FIRECRACKERS = 2,
    CHASE = 3,
    STROBE = 4,
    BIDI_CHASE = 5,
    ONESHOT_CHASE = 6,
    BIDI_ONESHOT_CHASE = 7,
    SPARKLE = 8,
    WAVE = 9,
    PIEZO_TRIGGER = 10,
    FLAME = 11,
    FLAME2 = 12,
    GRASS = 13,
    CIRCULAR_CHASE = 14,
    BATTERY_TEST = 15,
    RAINBOW = 16,
}