use super::{Active, Coordinator, Ending, Internal, Phase};
use crate::health::HealthProbe;
use crate::prepare::{self, PrepareEnd, PrepareOutcome, RerunRequest, StepOutcome};
use crate::process::{ExitCode, ExitWording, ManagedProcess, Priority, describe_exit};
use crate::{ServiceId, ServiceState};

enum Readiness {
    Satisfied,
    Pending(Vec<ServiceId>),
    Blocked(String),
}

impl Coordinator {
    pub(super) fn request_start(&mut self, service: usize) {
        if self.shutting_down {
            return;
        }
        match &self.slots[service].state {
            ServiceState::Stopped | ServiceState::Exited { .. } | ServiceState::Failed { .. } => {
                self.apply_readiness(service);
            }
            ServiceState::Stopping => self.stop(service, true),
            _ => {}
        }
    }

    pub(super) fn restart(&mut self, service: usize) {
        if self.slots[service].active.is_some() {
            self.stop(service, true);
        } else {
            self.request_start(service);
        }
    }

    pub(super) fn stop(&mut self, service: usize, then_start: bool) {
        self.slots[service].rebuild_during_first_prepare = RerunRequest::NotRequested;
        if matches!(self.slots[service].state, ServiceState::Waiting { .. }) {
            self.set_state(service, ServiceState::Stopped);
            return;
        }
        let Some(active) = &mut self.slots[service].active else {
            return;
        };
        let already_terminating = active.ending.is_some();
        let then_start = then_start || active.ending == Some(Ending::Stopping { then_start: true });
        active.ending = Some(Ending::Stopping { then_start });
        if !already_terminating {
            self.terminate(service);
        }
        self.set_state(service, ServiceState::Stopping);
    }

    pub(super) fn reconcile(&mut self) {
        let mut changed = true;
        while changed {
            changed = false;
            for position in 0..self.start_order.len() {
                let service = self.start_order[position];
                if matches!(self.slots[service].state, ServiceState::Waiting { .. }) {
                    changed |= self.apply_readiness(service);
                }
            }
        }
    }

    fn apply_readiness(&mut self, service: usize) -> bool {
        match self.readiness(service) {
            Readiness::Satisfied => {
                self.begin(service);
                true
            }
            Readiness::Pending(on) => self.set_state(service, ServiceState::Waiting { on }),
            Readiness::Blocked(reason) => self.set_state(service, ServiceState::Failed { reason }),
        }
    }

    fn readiness(&self, service: usize) -> Readiness {
        let mut pending = Vec::new();
        for &need in &self.slots[service].needs {
            let slot = &self.slots[need];
            match &slot.state {
                ServiceState::Ready { .. } => {}
                ServiceState::Running { .. } if slot.spec.ports.is_empty() => {}
                ServiceState::Exited { code } => {
                    return Readiness::Blocked(format!(
                        "needs {}, which {}",
                        slot.spec.id,
                        describe_exit(*code, ExitWording::Exited)
                    ));
                }
                ServiceState::Failed { .. } => {
                    return Readiness::Blocked(format!("needs {}, which failed", slot.spec.id));
                }
                _ => pending.push(slot.spec.id.clone()),
            }
        }
        if pending.is_empty() {
            Readiness::Satisfied
        } else {
            Readiness::Pending(pending)
        }
    }

    fn begin(&mut self, service: usize) {
        let slot = &self.slots[service];
        if slot.prepared || slot.spec.prepare.is_empty() {
            self.launch(service, Phase::Service);
        } else {
            self.launch(service, Phase::Prepare { step: 0 });
        }
    }

    fn launch(&mut self, service: usize, phase: Phase) {
        if self.shutting_down {
            self.set_state(service, ServiceState::Stopped);
            return;
        }
        let total = self.slots[service].spec.prepare.len();
        let command = match phase {
            Phase::Prepare { step } => {
                self.set_state(
                    service,
                    ServiceState::Preparing {
                        step: prepare::step_number(step),
                        total,
                    },
                );
                self.slots[service].spec.prepare[step].clone()
            }
            Phase::Service => {
                self.set_state(service, ServiceState::Starting);
                self.slots[service].spec.command.clone()
            }
        };
        let generation = self.next_generation();
        match self.spawn_process(
            service,
            generation,
            phase.source(),
            &command,
            Priority::Normal,
        ) {
            Ok(process) => self.track(service, generation, phase, process),
            Err(reason) => {
                self.set_state(
                    service,
                    ServiceState::Failed {
                        reason: launch_failure(phase, total, &reason),
                    },
                );
            }
        }
    }

    fn track(&mut self, service: usize, generation: u64, phase: Phase, process: ManagedProcess) {
        let pid = process.pid();
        let ports = &self.slots[service].spec.ports;
        let health = (phase == Phase::Service && !ports.is_empty()).then(|| {
            let ready = self.internal_sender.clone();
            HealthProbe::spawn(ports.clone(), self.options.health_interval, move || {
                let _ = ready.send(Internal::PortsReady {
                    service,
                    generation,
                });
            })
        });
        self.slots[service].active = Some(Active {
            generation,
            phase,
            process,
            ending: None,
            _health: health,
        });
        if phase == Phase::Service {
            self.set_state(service, ServiceState::Running { pid });
        }
    }

    fn terminate(&self, service: usize) {
        let Some(active) = &self.slots[service].active else {
            return;
        };
        self.terminate_group(service, active.generation, active.process.terminator());
    }

    pub(super) fn leader_exited(&mut self, service: usize) {
        let Some(active) = &mut self.slots[service].active else {
            return;
        };
        if active.ending.is_none() {
            active.ending = Some(Ending::Exiting);
            self.terminate(service);
        }
    }

    pub(super) fn ports_ready(&mut self, service: usize) {
        if let ServiceState::Running { pid } = self.slots[service].state {
            self.set_state(service, ServiceState::Ready { pid });
        }
    }

    pub(super) fn group_gone(&mut self, service: usize, code: ExitCode) {
        let Some(active) = self.slots[service].active.take() else {
            return;
        };
        match (active.ending, active.phase) {
            (Some(Ending::Stopping { then_start }), _) => {
                self.set_state(service, ServiceState::Stopped);
                if then_start {
                    self.request_start(service);
                }
            }
            (_, Phase::Prepare { step }) => self.prepare_finished(service, step, code),
            (_, Phase::Service) => {
                self.set_state(service, ServiceState::Exited { code });
            }
        }
    }

    fn prepare_finished(&mut self, service: usize, step: usize, code: ExitCode) {
        let total = self.slots[service].spec.prepare.len();
        match prepare::step_outcome(step, total, code) {
            StepOutcome::Next { step } => self.launch(service, Phase::Prepare { step }),
            StepOutcome::Ended(outcome) => {
                let rebuild = std::mem::take(&mut self.slots[service].rebuild_during_first_prepare);
                self.first_prepare_ended(service, rebuild.rerun_or_report(outcome));
            }
        }
    }

    fn first_prepare_ended(&mut self, service: usize, end: PrepareEnd) {
        match end {
            PrepareEnd::RerunFromFirstStep => self.launch(service, Phase::Prepare { step: 0 }),
            PrepareEnd::Report(PrepareOutcome::Succeeded) => {
                self.slots[service].prepared = true;
                self.launch(service, Phase::Service);
            }
            PrepareEnd::Report(PrepareOutcome::Failed(reason)) => {
                self.set_state(service, ServiceState::Failed { reason });
            }
        }
    }
}

fn launch_failure(phase: Phase, total: usize, reason: &str) -> String {
    match phase {
        Phase::Prepare { step } => prepare::could_not_start(step, total, reason),
        Phase::Service => format!("could not start: {reason}"),
    }
}
