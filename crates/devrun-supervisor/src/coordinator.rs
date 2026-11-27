mod lifecycle;
mod rebuilding;

use portable_pty::PtySize;
use tokio::sync::{mpsc, watch};

use crate::health::HealthProbe;
use crate::prepare::RerunRequest;
use crate::process::{ExitCode, GroupTerminator, ManagedProcess, Priority};
use crate::queue::Publisher;
use crate::rebuild::{RebuildRun, WindingDown};
use crate::spec::LaunchPlan;
use crate::usage::{self, UsageSampler};
use crate::{
    Command, Event, LaunchSpec, OutputSource, ServiceId, ServiceState, ServiceStatus,
    SupervisorOptions,
};

#[derive(Clone, Copy)]
struct Internal {
    service: usize,
    generation: u64,
    event: ProcessEvent,
}

#[derive(Clone, Copy)]
enum ProcessEvent {
    LeaderExited,
    GroupGone { code: ExitCode },
    PortsReady,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Service,
    Rebuild,
    WindingDownRebuild,
    Nobody,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PrepareStart {
    FirstPrepare,
    Rebuild,
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
    rebuild_during_first_prepare: RerunRequest,
    active: Option<Active>,
    rebuild: Option<RebuildRun>,
    winding_down: Vec<WindingDown>,
    awaiting_wind_down: Option<PrepareStart>,
}

impl Slot {
    fn owner(&self, generation: u64) -> Owner {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.generation == generation)
        {
            Owner::Service
        } else if self
            .rebuild
            .as_ref()
            .is_some_and(|run| run.generation() == generation)
        {
            Owner::Rebuild
        } else if self
            .winding_down
            .iter()
            .any(|run| run.generation() == generation)
        {
            Owner::WindingDownRebuild
        } else {
            Owner::Nobody
        }
    }

    fn has_processes(&self) -> bool {
        self.active.is_some() || self.rebuild.is_some() || !self.winding_down.is_empty()
    }
}

pub(crate) struct Coordinator {
    slots: Vec<Slot>,
    start_order: Vec<usize>,
    options: SupervisorOptions,
    size: PtySize,
    events: Publisher,
    snapshot: watch::Sender<Vec<ServiceStatus>>,
    internal_sender: mpsc::UnboundedSender<Internal>,
    internal: mpsc::UnboundedReceiver<Internal>,
    next_generation: u64,
    shutting_down: bool,
    sampler: Option<UsageSampler>,
}

impl Coordinator {
    pub fn new(plan: LaunchPlan, options: SupervisorOptions, events: Publisher) -> Self {
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
                rebuild_during_first_prepare: RerunRequest::NotRequested,
                active: None,
                rebuild: None,
                winding_down: Vec::new(),
                awaiting_wind_down: None,
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
            sampler: UsageSampler::new(options.usage_interval),
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
        let mut clock = self.sampler.as_ref().map(UsageSampler::clock);
        loop {
            tokio::select! {
                biased;
                Some(message) = self.internal.recv() => self.handle_internal(message),
                command = commands.recv() => match command {
                    Some(Command::Shutdown) | None => break,
                    Some(command) => self.handle_command(command),
                },
                () = usage::tick(clock.as_mut()) => self.publish_usage(),
            }
            self.reconcile();
        }
        self.shutdown().await;
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Start(id) => self.with_service(&id, |this, service| {
                this.cancel_rebuild_unless_serving(service);
                this.request_start(service);
            }),
            Command::Stop(id) => {
                self.with_rebuild_cancelled(&id, |this, service| this.stop(service, false));
            }
            Command::Restart(id) => self.with_rebuild_cancelled(&id, Self::restart),
            Command::Rebuild(id) => self.with_service(&id, Self::rebuild),
            Command::Resize { rows, cols } => self.resize(rows, cols),
            Command::Input { service, bytes } => self.with_service(&service, |this, service| {
                if let Some(active) = &this.slots[service].active {
                    active.process.write(bytes);
                }
            }),
            Command::Shutdown => {}
        }
    }

    fn publish_usage(&mut self) {
        let leaders: Vec<(usize, u32)> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(service, slot)| {
                slot.active
                    .as_ref()
                    .map(|active| (service, active.process.pid()))
            })
            .collect();
        let Some(sampler) = &mut self.sampler else {
            return;
        };
        let pids: Vec<u32> = leaders.iter().map(|&(_, pid)| pid).collect();
        let usages = sampler.sample(&pids);
        for (&(service, _), usage) in leaders.iter().zip(usages) {
            if let Some(usage) = usage {
                self.emit(Event::Usage {
                    service: self.slots[service].spec.id.clone(),
                    usage,
                });
            }
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

    fn with_rebuild_cancelled(&mut self, id: &ServiceId, action: impl FnOnce(&mut Self, usize)) {
        self.with_service(id, |this, service| {
            this.cancel_rebuild(service);
            action(this, service);
        });
    }

    fn handle_internal(&mut self, message: Internal) {
        let Internal {
            service,
            generation,
            event,
        } = message;
        match (self.slots[service].owner(generation), event) {
            (Owner::Service, ProcessEvent::LeaderExited) => self.leader_exited(service),
            (Owner::Service, ProcessEvent::GroupGone { code }) => self.group_gone(service, code),
            (Owner::Service, ProcessEvent::PortsReady) => self.ports_ready(service),
            (Owner::Rebuild, ProcessEvent::LeaderExited) => self.rebuild_leader_exited(service),
            (Owner::Rebuild, ProcessEvent::GroupGone { code }) => {
                self.rebuild_group_gone(service, code);
            }
            (Owner::WindingDownRebuild, ProcessEvent::GroupGone { .. }) => {
                self.rebuild_wound_down(service, generation);
            }
            (Owner::Rebuild | Owner::WindingDownRebuild, ProcessEvent::PortsReady)
            | (Owner::WindingDownRebuild, ProcessEvent::LeaderExited)
            | (Owner::Nobody, _) => {}
        }
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

    fn next_generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    fn spawn_process(
        &self,
        service: usize,
        generation: u64,
        source: OutputSource,
        command: &str,
        priority: Priority,
    ) -> Result<ManagedProcess, String> {
        let output = self
            .events
            .output_sink(self.slots[service].spec.id.clone(), source);
        let exit = self.internal_sender.clone();
        ManagedProcess::spawn(
            &self.slots[service].spec,
            command,
            priority,
            self.size,
            move |bytes| output.send(bytes),
            move || {
                let _ = exit.send(Internal {
                    service,
                    generation,
                    event: ProcessEvent::LeaderExited,
                });
            },
        )
    }

    fn terminate_group(&self, service: usize, generation: u64, terminator: GroupTerminator) {
        let grace_period = self.options.grace_period;
        let gone = self.internal_sender.clone();
        tokio::spawn(async move {
            let code = terminator.terminate(grace_period).await;
            let _ = gone.send(Internal {
                service,
                generation,
                event: ProcessEvent::GroupGone { code },
            });
        });
    }

    async fn shutdown(&mut self) {
        self.shutting_down = true;
        for service in 0..self.slots.len() {
            self.cancel_rebuild(service);
        }
        for service in 0..self.slots.len() {
            if matches!(self.slots[service].state, ServiceState::Waiting { .. }) {
                self.set_state(service, ServiceState::Stopped);
            }
        }
        for position in (0..self.start_order.len()).rev() {
            let service = self.start_order[position];
            self.stop(service, false);
            while self.slots[service].has_processes() {
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
        self.events.publish(event);
    }
}
