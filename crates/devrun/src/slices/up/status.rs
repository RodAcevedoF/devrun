use devrun_supervisor::{RebuildProgress, ServiceId, ServiceState, ServiceStatus};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
const GIB: u64 = 1024 * MIB;

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

pub fn describe_rebuild(progress: &RebuildProgress) -> String {
    match progress {
        RebuildProgress::Running { step, total } => format!("rebuilding {step}/{total}"),
        RebuildProgress::Succeeded => "rebuilt".to_owned(),
        RebuildProgress::Failed { reason } => format!("rebuild failed: {reason}"),
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

pub fn is_live(state: &ServiceState) -> bool {
    matches!(
        state,
        ServiceState::Preparing { .. }
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

pub fn cpu_share(permille: u32) -> String {
    format!("{}.{}%", permille / 10, permille % 10)
}

pub fn memory_size(bytes: u64) -> String {
    if bytes < MIB {
        format!("{} KB", bytes / KIB)
    } else if bytes < GIB {
        format!("{} MB", bytes / MIB)
    } else {
        let tenths = (u128::from(bytes) * 10 + u128::from(GIB / 2)) / u128::from(GIB);
        format!("{}.{} GB", tenths / 10, tenths % 10)
    }
}

fn join(ids: &[ServiceId]) -> String {
    ids.iter()
        .map(ServiceId::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = 1024 * MIB;

    #[test]
    fn cpu_share_shows_permille_as_a_percent_with_one_decimal() {
        assert_eq!(cpu_share(0), "0.0%");
        assert_eq!(cpu_share(7), "0.7%");
        assert_eq!(cpu_share(125), "12.5%");
        assert_eq!(cpu_share(1000), "100.0%");
        assert_eq!(cpu_share(2000), "200.0%");
    }

    #[test]
    fn memory_size_uses_kb_below_one_mib() {
        assert_eq!(memory_size(0), "0 KB");
        assert_eq!(memory_size(999), "0 KB");
        assert_eq!(memory_size(2048), "2 KB");
        assert_eq!(memory_size(MIB - 1), "1023 KB");
    }

    #[test]
    fn memory_size_uses_mb_from_one_mib_below_one_gib() {
        assert_eq!(memory_size(MIB), "1 MB");
        assert_eq!(memory_size(150 * MIB), "150 MB");
        assert_eq!(memory_size(GIB - 1), "1023 MB");
    }

    #[test]
    fn memory_size_uses_gb_with_one_decimal_from_one_gib() {
        assert_eq!(memory_size(GIB), "1.0 GB");
        assert_eq!(memory_size(GIB + GIB / 2), "1.5 GB");
        assert_eq!(memory_size(12 * GIB), "12.0 GB");
    }

    #[test]
    fn describe_rebuild_names_the_step_the_outcome_and_the_failure_reason() {
        assert_eq!(
            describe_rebuild(&RebuildProgress::Running { step: 1, total: 2 }),
            "rebuilding 1/2"
        );
        assert_eq!(describe_rebuild(&RebuildProgress::Succeeded), "rebuilt");
        assert_eq!(
            describe_rebuild(&RebuildProgress::Failed {
                reason: "prepare step 2/2 failed with code 4".to_owned()
            }),
            "rebuild failed: prepare step 2/2 failed with code 4"
        );
    }

    #[test]
    fn only_preparing_starting_running_and_ready_services_are_live() {
        let live = [
            ServiceState::Preparing { step: 1, total: 1 },
            ServiceState::Starting,
            ServiceState::Running { pid: 1 },
            ServiceState::Ready { pid: 1 },
        ];
        let idle = [
            ServiceState::Waiting { on: Vec::new() },
            ServiceState::Stopping,
            ServiceState::Stopped,
            ServiceState::Exited { code: Some(0) },
            ServiceState::Failed {
                reason: "boom".to_owned(),
            },
        ];

        assert!(live.iter().all(is_live));
        assert!(!idle.iter().any(is_live));
    }
}
