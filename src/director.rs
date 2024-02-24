use std::path::PathBuf;
use anyhow::Context;
use crossbeam_channel::Receiver;
use crossbeam_channel::RecvTimeoutError;
use midly::live::LiveEvent;
use midly::MidiMessage;
use std::fs::File;
use log::{debug,info,error};
use std::time::Duration;

use crate::show::ShowDefinition;
use crate::config::ConfigFile;
use crate::radio::Radio;
use crate::showstate::ShowState;

/// This module is where a lot of the action happens. MIDI message
/// meet show configuration to fire radio packets.

const RESET_CONTROLLER: u8 = 103;

pub enum DirectorMessage {
    /// deliver a payload of a midi event
    MidiMessage { ts: u64, buf: Vec<u8> },

    /// shut down the event loop and exit the run_show routine
    Shutdown,

    /// reload the show config and then reinitialize receivers and show state
    Reload,
}

pub struct Director {
    config: ConfigFile,
    radio: Radio,
    rx: Receiver<DirectorMessage>
}

impl Director {

    pub fn new(config: ConfigFile, radio: Radio, rx: Receiver<DirectorMessage>) -> Director {
        Director {
            config,
            radio,
            rx
        }
    }

    pub fn run_show(self: &mut Self) -> anyhow::Result<()> {
        let show_path = PathBuf::from(&self.config.show_file);
        debug!("Show path is: {:?}", show_path);
        'outer: loop {
            match self.load_and_run(&show_path) {
                Ok(false) => break 'outer,
                Err(e) => {
                    error!("Error loading/running show, waiting for reload command. Error: {:?}", e);
                    loop { match self.rx.recv()? {
                            DirectorMessage::Shutdown => break 'outer,
                            DirectorMessage::Reload => break,
                            _ => {}
                        }
                    }
                },
                _ => {}
            }
        }
        debug!("Exiting run_show");
        Ok(())
    }

    fn load_and_run(self: &Self, show_path: &PathBuf) -> anyhow::Result<bool> {
        let file = File::open(&show_path).context("Could not open file")?;
        let show = serde_json::from_reader::<File,ShowDefinition>(file).context("Could not parse file")?;
        let state = ShowState::new(&show, &self.radio, &self.config).context("Could not validate show structure")?;
        let mut mutable_state = state.create_mutable_state().context("Could not validate show structure")?;
        state.configure_receivers()?;

        info!("reset receivers and show state");
        let mut timeout = Duration::ZERO;
        loop {
            match self.rx.recv_timeout(timeout) {
                Ok(message) => {
                    match message {
                        DirectorMessage::Reload => return Ok(true),
                        DirectorMessage::Shutdown => return Ok(false),
                        DirectorMessage::MidiMessage { ts: _, buf } => {
                            let midi_event = midly::live::LiveEvent::parse(&buf)?;
                            if let LiveEvent::Midi{ channel, message } = midi_event {
                                if channel == self.config.midi_control_channel {
                                    if let MidiMessage::Controller { controller, value } = message {
                                        if controller == RESET_CONTROLLER && value == 127 {
                                            info!("midi reset received");
                                            return Ok(true)
                                        }
                                    }
                                }
                            }
                            state.process_midi(&midi_event, &mut mutable_state)?;
                        }
                    }
                }
                Err(e) => match e {
                    RecvTimeoutError::Timeout => {},
                    RecvTimeoutError::Disconnected => {
                        error!("channel closed, exiting show loop");
                        return Ok(false)
                    }
                }
            };
            timeout = state.tick(&mut mutable_state)?;
        }
    }

}