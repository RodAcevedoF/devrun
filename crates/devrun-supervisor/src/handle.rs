use tokio::sync::{mpsc, watch};

use crate::coordinator::Coordinator;
use crate::spec::LaunchPlan;
use crate::{Command, Event, LaunchSpec, ServiceStatus, SupervisorError, SupervisorOptions};

pub struct Supervisor;

impl Supervisor {
    pub fn spawn(
        specs: Vec<LaunchSpec>,
        options: SupervisorOptions,
    ) -> Result<SupervisorHandle, SupervisorError> {
        let plan = LaunchPlan::validate(specs)?;
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| SupervisorError::NoRuntime)?;
        let (command_sender, commands) = mpsc::unbounded_channel();
        let (event_sender, events) = mpsc::unbounded_channel();
        let coordinator = Coordinator::new(plan, options, event_sender);
        let snapshot = coordinator.subscribe();
        runtime.spawn(coordinator.run(commands));
        Ok(SupervisorHandle {
            commands: command_sender,
            events,
            snapshot,
        })
    }
}

pub struct SupervisorHandle {
    commands: mpsc::UnboundedSender<Command>,
    events: mpsc::UnboundedReceiver<Event>,
    snapshot: watch::Receiver<Vec<ServiceStatus>>,
}

impl SupervisorHandle {
    pub fn send(&self, command: Command) -> Result<(), SupervisorError> {
        self.commands
            .send(command)
            .map_err(|_| SupervisorError::Closed)
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    pub fn snapshot(&self) -> Vec<ServiceStatus> {
        self.snapshot.borrow().clone()
    }
}
