use crate::ServiceState;
use crate::prepare::{self, PrepareEnd, RerunRequest, StepOutcome};
use crate::process::{ExitCode, GroupTerminator, ManagedProcess};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceProcess {
    Serving,
    NotServing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RebuildRequest {
    RerunFirstPrepare,
    RunAlongsideService,
    PrepareOnNextStart,
    PrepareAndStart,
}

pub(crate) fn on_request(state: &ServiceState, process: ServiceProcess) -> RebuildRequest {
    match state {
        ServiceState::Preparing { .. } => RebuildRequest::RerunFirstPrepare,
        ServiceState::Starting | ServiceState::Running { .. } | ServiceState::Ready { .. } => {
            match process {
                ServiceProcess::Serving => RebuildRequest::RunAlongsideService,
                ServiceProcess::NotServing => RebuildRequest::PrepareOnNextStart,
            }
        }
        ServiceState::Exited { .. } | ServiceState::Failed { .. } => {
            RebuildRequest::PrepareAndStart
        }
        ServiceState::Waiting { .. } | ServiceState::Stopping | ServiceState::Stopped => {
            RebuildRequest::PrepareOnNextStart
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Running,
    Reaping,
}

pub(crate) enum StepEnd {
    NextStep { step: usize, rerun: RerunRequest },
    Ended(PrepareEnd),
}

pub(crate) struct RebuildRun {
    generation: u64,
    step: usize,
    process: ManagedProcess,
    rerun: RerunRequest,
    stage: Stage,
}

pub(crate) struct WindingDown {
    generation: u64,
    _owned_until_its_group_is_reaped: ManagedProcess,
}

impl WindingDown {
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl RebuildRun {
    pub fn new(generation: u64, step: usize, process: ManagedProcess, rerun: RerunRequest) -> Self {
        Self {
            generation,
            step,
            process,
            rerun,
            stage: Stage::Running,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn request_rerun(&mut self) {
        self.rerun = RerunRequest::Requested;
    }

    pub fn leader_exited(&mut self) -> Option<GroupTerminator> {
        match std::mem::replace(&mut self.stage, Stage::Reaping) {
            Stage::Running => Some(self.process.terminator()),
            Stage::Reaping => None,
        }
    }

    pub fn cancel(self) -> (WindingDown, Option<GroupTerminator>) {
        let terminator = match self.stage {
            Stage::Running => Some(self.process.terminator()),
            Stage::Reaping => None,
        };
        let winding_down = WindingDown {
            generation: self.generation,
            _owned_until_its_group_is_reaped: self.process,
        };
        (winding_down, terminator)
    }

    pub fn group_gone(self, total: usize, code: ExitCode) -> StepEnd {
        match prepare::step_outcome(self.step, total, code) {
            StepOutcome::Next { step } => StepEnd::NextStep {
                step,
                rerun: self.rerun,
            },
            StepOutcome::Ended(outcome) => StepEnd::Ended(self.rerun.rerun_or_report(outcome)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_service_rebuilds_alongside_only_while_its_process_keeps_serving() {
        for state in [
            ServiceState::Starting,
            ServiceState::Running { pid: 1 },
            ServiceState::Ready { pid: 1 },
        ] {
            assert_eq!(
                on_request(&state, ServiceProcess::Serving),
                RebuildRequest::RunAlongsideService
            );
            assert_eq!(
                on_request(&state, ServiceProcess::NotServing),
                RebuildRequest::PrepareOnNextStart
            );
        }
    }

    #[test]
    fn only_ended_services_recover_while_idle_ones_prepare_on_their_next_start() {
        let cases = [
            (
                ServiceState::Preparing { step: 1, total: 1 },
                RebuildRequest::RerunFirstPrepare,
            ),
            (
                ServiceState::Exited { code: Some(1) },
                RebuildRequest::PrepareAndStart,
            ),
            (
                ServiceState::Failed {
                    reason: "boom".to_owned(),
                },
                RebuildRequest::PrepareAndStart,
            ),
            (
                ServiceState::Waiting { on: Vec::new() },
                RebuildRequest::PrepareOnNextStart,
            ),
            (ServiceState::Stopping, RebuildRequest::PrepareOnNextStart),
            (ServiceState::Stopped, RebuildRequest::PrepareOnNextStart),
        ];
        for (state, expected) in cases {
            assert_eq!(on_request(&state, ServiceProcess::NotServing), expected);
        }
    }
}
