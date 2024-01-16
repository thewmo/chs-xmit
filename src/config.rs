use serde::Deserialize;

/// Mappings for a JSON config file that contains settings that are
/// not a property of a show, but rather the configuration of the
/// system (radio, etc). Notice details of modulation are hardcoded
/// because changing them would require a lot of testing and effort,
/// in addition to receiver changes
#[derive(Debug,Deserialize)]
pub struct ConfigFile {

    /// the path to the SPI device to open in the filesystem
    pub spi_device: String,

    /// the frequency to use expressed as a long
    pub frequency: u32,

    /// the id of this radio to use when transmitting.
    /// needs to be < 10 for the receivers to obey
    pub transmitter_id: u8,

    /// the transmitter power to use in dBm, between -18 and +20
    /// note that for most uses +17 is probably a good value as 
    /// it doesn't require toggling a "high power" state on/off
    /// during transmit
    pub transmitter_power: i8,

    /// amount of time to let the radio just be after
    /// resets etc, will use a default value if not supplied
    pub settle_time_millis: Option<u64>,

    /// the client name to pass to the midi library
    pub midi_client_name: String,

    /// the midi port to attach to for events. the string
    /// provided will be matched against the port name as a prefix
    pub midi_port: String,

    /// the path to the show file to load on startup
    pub show_file: String,

    /// the depth of buffer to use on the internal channel between
    /// the MIDI read thread and the main thread, will use a default
    /// value if none supplied
    pub channel_buf_depth: Option<usize>

}

