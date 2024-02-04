use std::path::PathBuf;
use anyhow::Context;
use crossbeam_channel::Receiver;
use crossbeam_channel::RecvTimeoutError;
use std::fs::File;
use log::{debug,info,error};
use std::time::Duration;

use crate::show::ShowDefinition;
use crate::config::ConfigFile;
use crate::radio::Radio;
use crate::showstate::ShowState;

/// This module is where a lot of the action happens. MIDI message
/// meet show configuration to fire radio packets.

pub const DEFAULT_TICK: Duration = Duration::from_secs(1);

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
        let mut state = ShowState::new(&show, &self.radio).context("Could not validate show structure")?;
        
        state.configure_receivers(&show)?;

        info!("Reset receivers and show state");
        let mut timeout = DEFAULT_TICK;
        loop {
            match self.rx.recv_timeout(timeout) {
                Ok(message) => {
                    match message {
                        DirectorMessage::Reload => return Ok(true),
                        DirectorMessage::Shutdown => return Ok(false),
                        DirectorMessage::MidiMessage { ts, buf } => {
                            timeout = state.process_midi(&show, ts, buf)?;
                        }
                    }
                }
                Err(e) => match e {
                    RecvTimeoutError::Timeout => {
                        timeout = state.process_tick()?;
                    },
                    RecvTimeoutError::Disconnected => {
                        error!("Channel closed, exiting show loop");
                        return Ok(false)
                    }
                }
            }
        }
    }

}