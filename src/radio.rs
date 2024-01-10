use log::debug;
use std::thread::sleep;
use std::time::Duration;
use rfm69::{Rfm69, registers::{Registers, Modulation, ModulationShaping, 
    ModulationType, DataMode, PacketConfig, PacketFormat, 
    PacketDc, PacketFiltering, InterPacketRxDelay, RxBw, RxBwFsk,
    Pa13dBm1, Pa13dBm2 }};
use linux_embedded_hal::spidev::{SpiModeFlags, SpidevOptions};
use linux_embedded_hal::sysfs_gpio::Direction;
use linux_embedded_hal::{Spidev, SysfsPin};

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
const SPI_DEVICE: &str = "/dev/spidev0.1";
// rpi rf69 bonnet connects reset to GPIO25
const RESET_PIN: u64 = 25;

const FREQ: u32 = 427_000_000; // 427 MHz
const BIT_RATE: u32 = 250_000; // 250 kbps
const FREQ_DEVIATION: u32 = 250_000; // 250 kHz
const PREAMBLE_LENGTH: u16 = 4;
const SYNCWORD: &str = "CHS";
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
const NODE_ADDRESS: u8 = 5;
const SETTLE_TIME: Duration = Duration::from_millis(10); // time to let the radio settle between config changes/resets

type MyRfm = Rfm69<rfm69::NoCs, rfm69::SpiTransactional<Spidev>>;

pub struct Radio {
    radio: MyRfm,
    my_address: u8,
    power: i8
}

impl Radio {
    pub fn init(my_address: u8, power: i8) -> Result<Radio, RadioError>  {

        // the rfm69 bonnet pulls the reset pin high by
        // default, it needs to be pulled low to bring the radio
        // out of reset
        let reset_pin = SysfsPin::new(RESET_PIN);
        reset_pin.export()?;

        // this will configure the pin as output and high (placing the radio in reset)
        reset_pin.set_direction(Direction::High)?;
        // let things stabilize for 10ms
        sleep(SETTLE_TIME);
        // turn on the radio by taking reset low
        reset_pin.set_value(0)?;
        // and again before trying to configure the radio
        sleep(SETTLE_TIME);

        let mut spi = Spidev::open(SPI_DEVICE).unwrap();
        let options = SpidevOptions::new()
            .bits_per_word(8)
            .max_speed_hz(1_000_000)
            .mode(SpiModeFlags::SPI_MODE_0)
            .build();
        spi.configure(&options)?;

        let mut radio = Rfm69::new_without_cs(spi);
        radio.modulation(Modulation { ..MODULATION })?;
        radio.sync(SYNCWORD.as_bytes())?;
        radio.frequency(FREQ)?;
        radio.bit_rate(BIT_RATE)?;
        radio.packet(PACKET_CONFIG)?;
        radio.fdev(FREQ_DEVIATION)?;
        radio.rx_bw(RX_BW)?;
        radio.rx_afc_bw(RX_BW)?;
        radio.node_address(NODE_ADDRESS)?;
        radio.preamble(PREAMBLE_LENGTH)?;
        radio.broadcast_address(0xFF)?;
        radio.fifo_mode(rfm69::registers::FifoMode::NotEmpty)?;

        // rfm69 power is confusing, there are two power amps that can each be enabled/disabled
        // (or combined) and a "high power" mode from 18-20 dBm requiring enabling/disabling as
        // part of each write.
        // good writeup at https://andrehessling.de/2015/02/07/figuring-out-the-power-level-settings-of-hoperfs-rfm69-hwhcw-modules/
        // tldr: If you use RFM69HW modules, enable PA1 (and only PA1!) for output powers less than +13 dBm. Combine PA1 and PA2 for powers 
        // between +13 dBm and +17 dBm. And only if you need more power, use PA1+PA2 with high power settings to get more than +17 dBm.
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
        Ok(Radio { radio, my_address, power })
    }

    pub fn send(self: &mut Self, packet: &Packet) -> Result<(),RadioError> {
       let high_power = (18..=20).contains(&self.power);
       if high_power {
            self.radio.write(Registers::Ocp, 0x0F)?; // disables over-current protection
            self.radio.pa13_dbm1(Pa13dBm1::High20dBm)?;
            self.radio.pa13_dbm2(Pa13dBm2::High20dBm)?;
        }
        let marshalled = packet.marshal(self.my_address, 0, 0);
        debug!("Sending packet: {:?}, marshalled: {:?}", packet, marshalled);
        self.radio.send(marshalled.as_slice())?;
 
        if high_power {
            self.radio.write(Registers::Ocp, 0x1A)?; // re-enables over-current protection
            self.radio.pa13_dbm1(Pa13dBm1::Normal)?;
            self.radio.pa13_dbm2(Pa13dBm2::Normal)?;
        }
        return Ok(())
    }

}

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