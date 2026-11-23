use thiserror::Error;

use crate::ServiceId;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SupervisorError {
    #[error("service `{0}` is defined more than once")]
    DuplicateService(ServiceId),
    #[error("service `{service}` needs unknown service `{need}`")]
    UnknownNeed { service: ServiceId, need: ServiceId },
    #[error("dependency cycle: {}", join(.0))]
    DependencyCycle(Vec<ServiceId>),
    #[error("supervisor must be spawned inside a tokio runtime")]
    NoRuntime,
    #[error("supervisor is no longer running")]
    Closed,
    #[error("cannot listen for termination signals: {0}")]
    Signals(String),
}

fn join(ids: &[ServiceId]) -> String {
    ids.iter()
        .map(ServiceId::as_str)
        .collect::<Vec<_>>()
        .join(" -> ")
}
