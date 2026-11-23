use std::thread;

use nix::sys::signal::{SigSet, Signal};
use tokio::sync::mpsc;

use crate::SupervisorError;

const TERMINATION: [Signal; 3] = [Signal::SIGINT, Signal::SIGTERM, Signal::SIGHUP];

pub struct TerminationSignals {
    set: SigSet,
}

impl TerminationSignals {
    pub fn block() -> Result<Self, SupervisorError> {
        let mut set = SigSet::empty();
        for signal in TERMINATION {
            set.add(signal);
        }
        set.thread_block()
            .map_err(|error| SupervisorError::Signals(error.to_string()))?;
        Ok(Self { set })
    }

    pub fn listen(self) -> mpsc::UnboundedReceiver<()> {
        let (sender, receiver) = mpsc::unbounded_channel();
        thread::spawn(move || {
            while self.set.wait().is_ok() {
                if sender.send(()).is_err() {
                    break;
                }
            }
        });
        receiver
    }
}
