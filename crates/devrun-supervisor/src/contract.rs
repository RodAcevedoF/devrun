use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ServiceId(String);

impl ServiceId {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServiceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    Start(ServiceId),
    Stop(ServiceId),
    Restart(ServiceId),
    Resize { rows: u16, cols: u16 },
    Input { service: ServiceId, bytes: Vec<u8> },
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputSource {
    Prepare,
    Service,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    State {
        service: ServiceId,
        state: ServiceState,
    },
    Output {
        service: ServiceId,
        source: OutputSource,
        bytes: Vec<u8>,
    },
    OutputDropped {
        service: ServiceId,
        bytes: u64,
    },
    Rejected {
        reason: String,
    },
    ShutdownComplete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServiceState {
    Waiting { on: Vec<ServiceId> },
    Preparing { step: usize, total: usize },
    Starting,
    Running { pid: u32 },
    Ready { pid: u32 },
    Stopping,
    Stopped,
    Exited { code: Option<i32> },
    Failed { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceStatus {
    pub id: ServiceId,
    pub state: ServiceState,
}
