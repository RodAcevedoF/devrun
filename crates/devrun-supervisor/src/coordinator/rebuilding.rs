use super::{Coordinator, Phase, PrepareStart};
use crate::prepare::{self, PrepareEnd, PrepareOutcome, RerunRequest};
use crate::process::{ExitCode, Priority};
use crate::rebuild::{self, RebuildRequest, RebuildRun, ServiceProcess, StepEnd};
use crate::{Event, OutputSource, RebuildProgress, ServiceState};

impl Coordinator {
    pub(super) fn rebuild(&mut self, service: usize) {
        if self.slots[service].awaiting_wind_down.is_some() {
            return;
        }
        if let Some(run) = &mut self.slots[service].rebuild {
            run.request_rerun();
            return;
        }
        match rebuild::on_request(&self.slots[service].state, self.service_process(service)) {
            RebuildRequest::RerunFirstPrepare => {
                self.slots[service].rebuild_during_first_prepare = RerunRequest::Requested;
            }
            RebuildRequest::RunAlongsideService => self.start_rebuild(service),
            RebuildRequest::PrepareOnNextStart => self.slots[service].prepared = false,
            RebuildRequest::PrepareAndStart => {
                self.slots[service].prepared = false;
                self.request_start(service);
            }
        }
    }

    pub(super) fn cancel_rebuild_unless_serving(&mut self, service: usize) {
        match self.service_process(service) {
            ServiceProcess::Serving => {}
            ServiceProcess::NotServing => self.cancel_rebuild(service),
        }
    }

    pub(super) fn cancel_rebuild(&mut self, service: usize) {
        let slot = &mut self.slots[service];
        slot.awaiting_wind_down
            .take_if(|start| *start == PrepareStart::Rebuild);
        let Some(run) = slot.rebuild.take() else {
            return;
        };
        let (winding_down, terminator) = run.cancel();
        if let Some(terminator) = terminator {
            self.terminate_group(service, winding_down.generation(), terminator);
        }
        let slot = &mut self.slots[service];
        slot.prepared = false;
        slot.winding_down.push(winding_down);
    }

    pub(super) fn rebuild_leader_exited(&mut self, service: usize) {
        let Some(run) = &mut self.slots[service].rebuild else {
            return;
        };
        let generation = run.generation();
        if let Some(terminator) = run.leader_exited() {
            self.terminate_group(service, generation, terminator);
        }
    }

    pub(super) fn rebuild_group_gone(&mut self, service: usize, code: ExitCode) {
        let Some(run) = self.slots[service].rebuild.take() else {
            return;
        };
        let total = self.slots[service].spec.prepare.len();
        match run.group_gone(total, code) {
            StepEnd::NextStep { step, rerun } => self.launch_rebuild_step(service, step, rerun),
            StepEnd::Ended(end) => self.rebuild_ended(service, end),
        }
    }

    pub(super) fn rebuild_wound_down(&mut self, service: usize, generation: u64) {
        let slot = &mut self.slots[service];
        slot.winding_down
            .retain(|run| run.generation() != generation);
        if slot.winding_down.is_empty()
            && let Some(start) = slot.awaiting_wind_down.take()
        {
            self.launch_prepare(service, start);
        }
    }

    pub(super) fn start_prepare(&mut self, service: usize, start: PrepareStart) {
        if self.slots[service].winding_down.is_empty() {
            self.launch_prepare(service, start);
        } else {
            self.announce_awaited_prepare(service, start);
            self.slots[service].awaiting_wind_down = Some(start);
        }
    }

    fn launch_prepare(&mut self, service: usize, start: PrepareStart) {
        match start {
            PrepareStart::FirstPrepare => self.launch(service, Phase::Prepare { step: 0 }),
            PrepareStart::Rebuild => {
                self.launch_rebuild_step(service, 0, RerunRequest::NotRequested);
            }
        }
    }

    fn announce_awaited_prepare(&mut self, service: usize, start: PrepareStart) {
        match start {
            PrepareStart::FirstPrepare => {
                let total = self.slots[service].spec.prepare.len();
                self.set_state(
                    service,
                    ServiceState::Preparing {
                        step: prepare::step_number(0),
                        total,
                    },
                );
            }
            PrepareStart::Rebuild => {}
        }
    }

    fn service_process(&self, service: usize) -> ServiceProcess {
        let serving = self.slots[service]
            .active
            .as_ref()
            .is_some_and(|active| active.phase == Phase::Service && active.ending.is_none());
        if serving {
            ServiceProcess::Serving
        } else {
            ServiceProcess::NotServing
        }
    }

    fn start_rebuild(&mut self, service: usize) {
        if self.slots[service].spec.prepare.is_empty() {
            self.rebuild_ended(service, PrepareEnd::Report(PrepareOutcome::Succeeded));
        } else {
            self.start_prepare(service, PrepareStart::Rebuild);
        }
    }

    fn launch_rebuild_step(&mut self, service: usize, step: usize, rerun: RerunRequest) {
        let total = self.slots[service].spec.prepare.len();
        self.emit_rebuild(
            service,
            RebuildProgress::Running {
                step: prepare::step_number(step),
                total,
            },
        );
        let command = self.slots[service].spec.prepare[step].clone();
        let generation = self.next_generation();
        match self.spawn_process(
            service,
            generation,
            OutputSource::Prepare,
            &command,
            Priority::Low,
        ) {
            Ok(process) => {
                self.slots[service].rebuild =
                    Some(RebuildRun::new(generation, step, process, rerun));
            }
            Err(reason) => {
                let outcome =
                    PrepareOutcome::Failed(prepare::could_not_start(step, total, &reason));
                self.rebuild_ended(service, rerun.rerun_or_report(outcome));
            }
        }
    }

    fn rebuild_ended(&mut self, service: usize, end: PrepareEnd) {
        match end {
            PrepareEnd::RerunFromFirstStep => self.start_rebuild(service),
            PrepareEnd::Report(PrepareOutcome::Succeeded) => {
                self.slots[service].prepared = true;
                self.emit_rebuild(service, RebuildProgress::Succeeded);
                self.restart(service);
            }
            PrepareEnd::Report(PrepareOutcome::Failed(reason)) => {
                self.slots[service].prepared = false;
                self.emit_rebuild(service, RebuildProgress::Failed { reason });
            }
        }
    }

    fn emit_rebuild(&self, service: usize, progress: RebuildProgress) {
        self.emit(Event::Rebuild {
            service: self.slots[service].spec.id.clone(),
            progress,
        });
    }
}
