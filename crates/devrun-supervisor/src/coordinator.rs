use portable_pty::PtySize;
use tokio::sync::{mpsc, watch};

use crate::health::HealthProbe;
use crate::process::{ExitCode, ManagedProcess};
use crate::spec::LaunchPlan;
use crate::{
    Command, Event, LaunchSpec, OutputSource, ServiceId, ServiceState, ServiceStatus,
    SupervisorOptions,
};

enum Internal {
    Output {
        service: usize,
        source: OutputSource,
        bytes: Vec<u8>,
    },
    LeaderExited {
        service: usize,
        generation: u64,
    },
    GroupGone {
        service: usize,
        generation: u64,
        code: ExitCode,
    },
    PortsReady {
        service: usize,
        generation: u64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Prepare { step: usize },
    Service,
}

impl Phase {
    fn source(self) -> OutputSource {
        match self {
            Self::Prepare { .. } => OutputSource::Prepare,
            Self::Service => OutputSource::Service,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Exiting,
    Stopping { then_start: bool },
}

struct Active {
    generation: u64,
    phase: Phase,
    process: ManagedProcess,
    ending: Option<Ending>,
    _health: Option<HealthProbe>,
}

struct Slot {
    spec: LaunchSpec,
    needs: Vec<usize>,
    state: ServiceState,
    prepared: bool,
    active: Option<Active>,
}

enum Readiness {
    Satisfied,
    Pending(Vec<ServiceId>),
    Blocked(String),
}

pub(crate) struct Coordinator {
    slots: Vec<Slot>,
    start_order: Vec<usize>,
    options: SupervisorOptions,
    size: PtySize,
    events: mpsc::UnboundedSender<Event>,
    snapshot: watch::Sender<Vec<ServiceStatus>>,
    internal_sender: mpsc::UnboundedSender<Internal>,
    internal: mpsc::UnboundedReceiver<Internal>,
    next_generation: u64,
    shutting_down: bool,
}

impl Coordinator {
    pub fn new(
        plan: LaunchPlan,
        options: SupervisorOptions,
        events: mpsc::UnboundedSender<Event>,
    ) -> Self {
        let slots: Vec<Slot> = plan
            .specs
            .into_iter()
            .zip(plan.needs)
            .map(|(spec, needs)| Slot {
                state: ServiceState::Waiting {
                    on: spec.needs.clone(),
                },
                spec,
                needs,
                prepared: false,
                active: None,
            })
            .collect();
        let statuses = slots
            .iter()
            .map(|slot| ServiceStatus {
                id: slot.spec.id.clone(),
                state: slot.state.clone(),
            })
            .collect();
        let (internal_sender, internal) = mpsc::unbounded_channel();
        Self {
            slots,
            start_order: plan.start_order,
            size: PtySize {
                rows: options.rows,
                cols: options.cols,
                ..PtySize::default()
            },
            options,
            events,
            snapshot: watch::Sender::new(statuses),
            internal_sender,
            internal,
            next_generation: 0,
            shutting_down: false,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<Vec<ServiceStatus>> {
        self.snapshot.subscribe()
    }

    pub async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        self.reconcile();
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(Command::Shutdown) | None => break,
                    Some(command) => self.handle_command(command),
                },
                Some(message) = self.internal.recv() => self.handle_internal(message),
            }
            self.reconcile();
        }
        self.shutdown().await;
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Start(id) => self.with_service(&id, Self::request_start),
            Command::Stop(id) => self.with_service(&id, |this, service| this.stop(service, false)),
            Command::Restart(id) => self.with_service(&id, Self::restart),
            Command::Resize { rows, cols } => self.resize(rows, cols),
            Command::Input { service, bytes } => self.with_service(&service, |this, service| {
                if let Some(active) = &this.slots[service].active {
                    active.process.write(bytes);
                }
            }),
            Command::Shutdown => {}
        }
    }

    fn with_service(&mut self, id: &ServiceId, action: impl FnOnce(&mut Self, usize)) {
        match self.slots.iter().position(|slot| slot.spec.id == *id) {
            Some(service) => action(self, service),
            None => self.emit(Event::Rejected {
                reason: format!("unknown service `{id}`"),
            }),
        }
    }

    fn handle_internal(&mut self, message: Internal) {
        match message {
            Internal::Output {
                service,
                source,
                bytes,
            } => self.emit(Event::Output {
                service: self.slots[service].spec.id.clone(),
                source,
                bytes,
            }),
            Internal::LeaderExited {
                service,
                generation,
            } => self.leader_exited(service, generation),
            Internal::GroupGone {
                service,
                generation,
                code,
            } => self.group_gone(service, generation, code),
            Internal::PortsReady {
                service,
                generation,
            } => self.ports_ready(service, generation),
        }
    }

    fn request_start(&mut self, service: usize) {
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

    fn restart(&mut self, service: usize) {
        if self.slots[service].active.is_some() {
            self.stop(service, true);
        } else {
            self.request_start(service);
        }
    }

    fn stop(&mut self, service: usize, then_start: bool) {
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

    fn resize(&mut self, rows: u16, cols: u16) {
        self.size = PtySize {
            rows,
            cols,
            ..PtySize::default()
        };
        for active in self.slots.iter().filter_map(|slot| slot.active.as_ref()) {
            active.process.resize(self.size);
        }
    }

    fn reconcile(&mut self) {
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
                        exit_description(*code)
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
                        step: step + 1,
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
        self.next_generation += 1;
        let generation = self.next_generation;
        match self.spawn_process(service, generation, phase, &command) {
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

    fn spawn_process(
        &self,
        service: usize,
        generation: u64,
        phase: Phase,
        command: &str,
    ) -> Result<ManagedProcess, String> {
        let output = self.internal_sender.clone();
        let exit = self.internal_sender.clone();
        let source = phase.source();
        ManagedProcess::spawn(
            &self.slots[service].spec,
            command,
            self.size,
            move |bytes| {
                let _ = output.send(Internal::Output {
                    service,
                    source,
                    bytes,
                });
            },
            move || {
                let _ = exit.send(Internal::LeaderExited {
                    service,
                    generation,
                });
            },
        )
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
        let terminator = active.process.terminator();
        let generation = active.generation;
        let grace_period = self.options.grace_period;
        let gone = self.internal_sender.clone();
        tokio::spawn(async move {
            let code = terminator.terminate(grace_period).await;
            let _ = gone.send(Internal::GroupGone {
                service,
                generation,
                code,
            });
        });
    }

    fn active_generation(&mut self, service: usize, generation: u64) -> Option<&mut Active> {
        self.slots[service]
            .active
            .as_mut()
            .filter(|active| active.generation == generation)
    }

    fn leader_exited(&mut self, service: usize, generation: u64) {
        let Some(active) = self.active_generation(service, generation) else {
            return;
        };
        if active.ending.is_none() {
            active.ending = Some(Ending::Exiting);
            self.terminate(service);
        }
    }

    fn ports_ready(&mut self, service: usize, generation: u64) {
        if self.active_generation(service, generation).is_none() {
            return;
        }
        if let ServiceState::Running { pid } = self.slots[service].state {
            self.set_state(service, ServiceState::Ready { pid });
        }
    }

    fn group_gone(&mut self, service: usize, generation: u64, code: ExitCode) {
        let Some(active) = self.slots[service]
            .active
            .take_if(|active| active.generation == generation)
        else {
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
        if code != Some(0) {
            let reason = format!(
                "prepare step {}/{total} {}",
                step + 1,
                failure_description(code)
            );
            self.set_state(service, ServiceState::Failed { reason });
        } else if step + 1 < total {
            self.launch(service, Phase::Prepare { step: step + 1 });
        } else {
            self.slots[service].prepared = true;
            self.launch(service, Phase::Service);
        }
    }

    async fn shutdown(&mut self) {
        self.shutting_down = true;
        for service in 0..self.slots.len() {
            if matches!(self.slots[service].state, ServiceState::Waiting { .. }) {
                self.set_state(service, ServiceState::Stopped);
            }
        }
        for position in (0..self.start_order.len()).rev() {
            let service = self.start_order[position];
            self.stop(service, false);
            while self.slots[service].active.is_some() {
                match self.internal.recv().await {
                    Some(message) => self.handle_internal(message),
                    None => break,
                }
            }
        }
        while let Ok(message) = self.internal.try_recv() {
            self.handle_internal(message);
        }
        self.emit(Event::ShutdownComplete);
    }

    fn set_state(&mut self, service: usize, state: ServiceState) -> bool {
        if self.slots[service].state == state {
            return false;
        }
        self.slots[service].state = state.clone();
        self.snapshot
            .send_modify(|statuses| statuses[service].state = state.clone());
        self.emit(Event::State {
            service: self.slots[service].spec.id.clone(),
            state,
        });
        true
    }

    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
    }
}

fn exit_description(code: ExitCode) -> String {
    match code {
        Some(code) => format!("exited with code {code}"),
        None => "was terminated by a signal".to_owned(),
    }
}

fn failure_description(code: ExitCode) -> String {
    match code {
        Some(code) => format!("failed with code {code}"),
        None => "was terminated by a signal".to_owned(),
    }
}

fn launch_failure(phase: Phase, total: usize, reason: &str) -> String {
    match phase {
        Phase::Prepare { step } => format!(
            "prepare step {}/{total} could not start: {reason}",
            step + 1
        ),
        Phase::Service => format!("could not start: {reason}"),
    }
}
