use log::debug;
use std::{cell::{Cell, RefCell}, num::Wrapping, thread::sleep};
use rfm69::{Rfm69, registers::{Registers, Modulation, ModulationShaping, 
    ModulationType, DataMode, PacketConfig, PacketFormat, 
    PacketDc, PacketFiltering, InterPacketRxDelay, RxBw, RxBwFsk,
    Pa13dBm1, Pa13dBm2 }};
use linux_embedded_hal::spidev::{SpiModeFlags, SpidevOptions};
use linux_embedded_hal::sysfs_gpio::Direction;
use linux_embedded_hal::{Spidev, SysfsPin};
use std::time::Duration;
use std::fmt::{Display,Formatter};

use crate::config::ConfigFile;
use crate::packet::Packet;

// reference links
// radio datasheet: https://cdn.sparkfun.com/datasheets/Wireless/General/RFM69HCW-V1.1.pdf
// radiohead MODEM_CONFIG_TABLE at https://github.com/adafruit/RadioHead/blob/master/RH_RF69.cpp
// Band receivers are using GFSK_Rb250Fd250 modem config as defined in radiohead
// numbers below are register numbers
// GFSK (BT=1.0), No Manchester, whitening, CRC, no address filtering
// AFC BW == RX BW == 2 x bit rate
//  02,           03,   04,   05,   06,   19,   1a,   37
// { CONFIG_GFSK, 0x00, 0x80, 0x10, 0x00, 0xe0, 0xe0, CONFIG_WHITE}, // GFSK_Rb250Fd250

// rpi rf69 bonnet uses chip select CE1 (the ".1" suffix here)
//const SPI_DEVICE: &str = "/dev/spidev0.1";

// rpi rf69 bonnet connects reset to GPIO25
const RESET_PIN: u64 = 25;

const BIT_RATE: u32 = 250_000; // 250 kbps
const FREQ_DEVIATION: u32 = 250_000; // 250 kHz
const PREAMBLE_LENGTH: u16 = 4;
const SYNCWORD: &str = "CHS";
const DEFAULT_SETTLE_TIME: u64 = 10;

const MODULATION: Modulation = Modulation { 
    data_mode: DataMode::Packet, 
    modulation_type: ModulationType::Fsk,
    shaping: ModulationShaping::Shaping01}; // shaping -> gaussian BT=1.0
const PACKET_CONFIG: PacketConfig = PacketConfig {
    format: PacketFormat::Variable(0xFFu8),
    dc: PacketDc::Whitening,
    crc: true,
    filtering: PacketFiltering::None,
    interpacket_rx_delay: InterPacketRxDelay::Delay1Bit,
    auto_rx_restart: true
};
const RX_BW: RxBw<RxBwFsk> = RxBw {
    dcc_cutoff: rfm69::registers::DccCutoff::Percent0dot125,
    rx_bw: RxBwFsk::Khz500dot0
};

type MyRfm = Rfm69<rfm69::NoCs, rfm69::SpiTransactional<Spidev>>;

pub struct Radio {
    // putting the radio in a refcell allows us to call mut methods on it without
    // having a mutable radio, which otherwise percolates up the encapsulation stack
    // and causes pain
    radio: RefCell<MyRfm>,
    my_address: u8,
    power: i8,
    packet_id: Cell<Wrapping<u8>>
}

impl Radio {
    pub fn init(config: &ConfigFile) -> Result<Radio, RadioError>  {

        // the rfm69 bonnet pulls the reset pin high by
        // default, it needs to be pulled low to bring the radio
        // out of reset
        let reset_pin = SysfsPin::new(RESET_PIN);
        reset_pin.export()?;
        // the first time we run after a reboot, the export takes some time te be
        // effective - otherwise a permissions error will result from the call below.
        // so we have to sleep a little bit. See https://github.com/rust-embedded/rust-sysfs-gpio/issues/5
        sleep(Duration::from_millis(100));

        // this will configure the pin as output and high (placing the radio in reset)
        reset_pin.set_direction(Direction::High)?;
        let settle_time = Duration::from_millis(config.settle_time_millis.unwrap_or(DEFAULT_SETTLE_TIME));
        // let things stabilize for 10ms
        sleep(settle_time);
        // turn on the radio by taking reset low
        reset_pin.set_value(0)?;
        // and again before trying to configure the radio
        sleep(settle_time);

        let mut spi = Spidev::open(&config.spi_device)?;
        let options = SpidevOptions::new()
            .bits_per_word(8)
            .max_speed_hz(1_000_000)
            .mode(SpiModeFlags::SPI_MODE_0)
            .build();
        spi.configure(&options)?;

        let mut radio = Rfm69::new_without_cs(spi);
        radio.modulation(Modulation { ..MODULATION })?;
        radio.sync(SYNCWORD.as_bytes())?;
        radio.frequency(config.frequency)?;
        radio.bit_rate(BIT_RATE)?;
        radio.packet(PACKET_CONFIG)?;
        radio.fdev(FREQ_DEVIATION)?;
        radio.rx_bw(RX_BW)?;
        radio.rx_afc_bw(RX_BW)?;
        radio.node_address(config.transmitter_id)?;
        radio.preamble(PREAMBLE_LENGTH)?;
        radio.broadcast_address(0xFF)?;
        radio.fifo_mode(rfm69::registers::FifoMode::NotEmpty)?;

        // rfm69 power is confusing, there are two power amps that can each be enabled/disabled
        // (or combined) and a "high power" mode from 18-20 dBm requiring enabling/disabling as
        // part of each write.
        // good writeup at https://andrehessling.de/2015/02/07/figuring-out-the-power-level-settings-of-hoperfs-rfm69-hwhcw-modules/
        // tldr: If you use RFM69HW modules, enable PA1 (and only PA1!) for output powers less than +13 dBm. Combine PA1 and PA2 for powers 
        // between +13 dBm and +17 dBm. And only if you need more power, use PA1+PA2 with high power settings to get more than +17 dBm.
        let power = config.transmitter_power;
        let pa_level: u8 = match power {
            -18..=13 => (power + 18) as u8 | 0x40, // 0x40 - PA1 only
            14..=17 => (power + 14) as u8 | 0x60, // 0x60 - PA1 + PA2 
            18..=20 => (power + 11) as u8 | 0x60, // PA1 + PA2 and enable "high power" on xmit
            _ => return Result::Err(RadioError::IllegalPower)
        };
        radio.write(Registers::PaLevel, pa_level)?;

        // now let's read back data from all the registers to confirm that the radio
        // is in fact alive and took our settings
        // Print content of all RFM registers
        for (index, val) in radio.read_all_regs()?.iter().enumerate() {
            debug!("Register 0x{:02x} = 0x{:02x}", index + 1, val);
        }
        Ok(Radio { radio: RefCell::new(radio), 
            my_address: config.transmitter_id, 
            power,
            packet_id: Cell::new(Wrapping(0u8)) })
    }

    pub fn send(self: &Self, packet: &Packet) -> Result<(),RadioError> {
        self.pre_tx_hook()?;
        let marshalled = packet.marshal(self.my_address, self.packet_id.get().0, 0);
        debug!("Sending packet: {:?}, marshalled: {:?}", packet, marshalled);
        let result = self.radio.borrow_mut().send(marshalled.as_slice());
        self.post_tx_hook()?;
        self.packet_id.set(self.packet_id.get() + Wrapping(1u8));
        result.map_err(From::from)
    }

    fn pre_tx_hook(self: &Self) -> Result<(),RadioError> {
        if (18..=20).contains(&self.power) {
            let mut rad = self.radio.borrow_mut();
            rad.write(Registers::Ocp, 0x0F)?; // disables over-current protection
            rad.pa13_dbm1(Pa13dBm1::High20dBm)?;
            rad.pa13_dbm2(Pa13dBm2::High20dBm)?;
        }
        return Ok(())
    }

    fn post_tx_hook(self: &Self) -> Result<(),RadioError> {
        let mut rad = self.radio.borrow_mut();
        if (18..=20).contains(&self.power) {
            rad.write(Registers::Ocp, 0x1A)?; // re-enables over-current protection
            rad.pa13_dbm1(Pa13dBm1::Normal)?;
            rad.pa13_dbm2(Pa13dBm2::Normal)?;
        }
        return Ok(())
    }

}

/// our own error type to wrap the underlying errors, not 
/// all of which implement the standard error trait, frustratingly
#[derive(Debug)]
pub enum RadioError {   
    SysfsError(linux_embedded_hal::sysfs_gpio::Error),
    Rfm69Error(Rfm69Error),
    SpiError(std::io::Error),
    IllegalPower
}

/// our own non-generic Rfm69Error type that can be fromable
#[derive(Debug)]
pub enum Rfm69Error {
    Cs,
    Spi,
    Timeout,
    AesKeySize,
    SyncSize,
    BufferTooSmall,
    PacketTooLarge,
}

impl<Ecs,Espi> From<rfm69::Error<Ecs,Espi>> for RadioError {
    fn from(err: rfm69::Error<Ecs,Espi>) -> RadioError {
        match err {
            rfm69::Error::Cs(_) => RadioError::Rfm69Error(Rfm69Error::Cs),
            rfm69::Error::Spi(_) => RadioError::Rfm69Error(Rfm69Error::Spi),
            rfm69::Error::Timeout => RadioError::Rfm69Error(Rfm69Error::Timeout),
            rfm69::Error::AesKeySize => RadioError::Rfm69Error(Rfm69Error::AesKeySize),
            rfm69::Error::SyncSize => RadioError::Rfm69Error(Rfm69Error::SyncSize),
            rfm69::Error::BufferTooSmall => RadioError::Rfm69Error(Rfm69Error::BufferTooSmall),
            rfm69::Error::PacketTooLarge => RadioError::Rfm69Error(Rfm69Error::PacketTooLarge)
        }
    }
}

impl From<linux_embedded_hal::sysfs_gpio::Error> for RadioError {
    fn from(err: linux_embedded_hal::sysfs_gpio::Error) -> RadioError {
        RadioError::SysfsError(err)
    }
}

impl From<std::io::Error> for RadioError {
    fn from(err: std::io::Error) -> RadioError {
        RadioError::SpiError(err)
    }
}

impl Display for RadioError {
    fn fmt(self: &Self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match &self {
            RadioError::SysfsError(e) => write!(f, "SysfsError: {:?}", e),
            RadioError::Rfm69Error(e) => write!(f, "Rfm69Error: {:?}", e),
            RadioError::SpiError(e) => write!(f, "SpiError: {:?}", e),
            RadioError::IllegalPower => write!(f, "Unsupported power value specified")
        }
    }
}

impl std::error::Error for RadioError {}
