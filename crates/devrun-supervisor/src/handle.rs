use tokio::sync::{mpsc, watch};

use crate::coordinator::Coordinator;
use crate::queue::{EventQueue, event_queue};
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
        let (publisher, events) = event_queue(options.output_buffer_bytes);
        let coordinator = Coordinator::new(plan, options, publisher);
        let snapshot = coordinator.subscribe();
        runtime.spawn(coordinator.run(commands));
        Ok(SupervisorHandle {
            commands: CommandSender(command_sender),
            events,
            snapshot,
        })
    }
}

#[derive(Clone)]
pub struct CommandSender(mpsc::UnboundedSender<Command>);

impl CommandSender {
    pub fn send(&self, command: Command) -> Result<(), SupervisorError> {
        self.0.send(command).map_err(|_| SupervisorError::Closed)
    }
}

pub struct SupervisorHandle {
    commands: CommandSender,
    events: EventQueue,
    snapshot: watch::Receiver<Vec<ServiceStatus>>,
}

impl SupervisorHandle {
    pub fn send(&self, command: Command) -> Result<(), SupervisorError> {
        self.commands.send(command)
    }

    pub fn commands(&self) -> CommandSender {
        self.commands.clone()
    }

    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }

    pub fn try_recv(&mut self) -> Option<Event> {
        self.events.try_recv()
    }

    pub fn snapshot(&self) -> Vec<ServiceStatus> {
        self.snapshot.borrow().clone()
    }
}
