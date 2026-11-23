use devrun_supervisor::{ServiceId, ServiceState, ServiceStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Idle,
    Preparing,
    Active,
    Ready,
    Failed,
    Ended,
}

pub fn tone(state: &ServiceState) -> Tone {
    match state {
        ServiceState::Waiting { .. } => Tone::Idle,
        ServiceState::Preparing { .. } => Tone::Preparing,
        ServiceState::Starting | ServiceState::Running { .. } => Tone::Active,
        ServiceState::Ready { .. } => Tone::Ready,
        ServiceState::Failed { .. } => Tone::Failed,
        ServiceState::Stopping | ServiceState::Stopped | ServiceState::Exited { .. } => Tone::Ended,
    }
}

pub fn badge(state: &ServiceState) -> &'static str {
    match state {
        ServiceState::Waiting { .. } => "waiting",
        ServiceState::Preparing { .. } => "preparing",
        ServiceState::Starting => "starting",
        ServiceState::Running { .. } => "running",
        ServiceState::Ready { .. } => "ready",
        ServiceState::Stopping => "stopping",
        ServiceState::Stopped => "stopped",
        ServiceState::Exited { .. } => "exited",
        ServiceState::Failed { .. } => "failed",
    }
}

pub fn describe(state: &ServiceState) -> String {
    match state {
        ServiceState::Waiting { on } if on.is_empty() => "waiting".to_owned(),
        ServiceState::Waiting { on } => format!("waiting on {}", join(on)),
        ServiceState::Preparing { step, total } => format!("preparing {step}/{total}"),
        ServiceState::Exited { code: Some(code) } => format!("exited with code {code}"),
        ServiceState::Exited { code: None } => "terminated by a signal".to_owned(),
        ServiceState::Failed { reason } => format!("failed: {reason}"),
        other => badge(other).to_owned(),
    }
}

pub fn is_active(state: &ServiceState) -> bool {
    matches!(
        state,
        ServiceState::Waiting { .. }
            | ServiceState::Preparing { .. }
            | ServiceState::Starting
            | ServiceState::Running { .. }
            | ServiceState::Ready { .. }
    )
}

pub fn has_ended(state: &ServiceState) -> bool {
    matches!(
        state,
        ServiceState::Stopped | ServiceState::Exited { .. } | ServiceState::Failed { .. }
    )
}

pub fn finished_cleanly(state: &ServiceState) -> bool {
    *state == ServiceState::Exited { code: Some(0) }
}

pub fn ready_count(statuses: &[ServiceStatus]) -> usize {
    statuses
        .iter()
        .filter(|status| matches!(status.state, ServiceState::Ready { .. }))
        .count()
}

pub fn services_noun(count: usize) -> &'static str {
    if count == 1 { "service" } else { "services" }
}

fn join(ids: &[ServiceId]) -> String {
    ids.iter()
        .map(ServiceId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}
