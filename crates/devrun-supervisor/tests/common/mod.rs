#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use devrun_supervisor::{
    Command, Event, LaunchSpec, OutputSource, ServiceId, ServiceState, Supervisor,
    SupervisorHandle, SupervisorOptions,
};
use nix::sys::signal::kill;
use nix::unistd::Pid;
use tokio::time::{sleep, timeout};

pub const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn id(raw: &str) -> ServiceId {
    ServiceId::new(raw)
}

pub fn spec(raw_id: &str, cwd: &Path, command: &str) -> LaunchSpec {
    LaunchSpec::new(id(raw_id), cwd, command)
}

pub fn options() -> SupervisorOptions {
    SupervisorOptions {
        grace_period: Duration::from_millis(300),
        health_interval: Duration::from_millis(50),
        ..SupervisorOptions::default()
    }
}

pub fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

pub fn process_exists(pid: u32) -> bool {
    kill(Pid::from_raw(i32::try_from(pid).unwrap()), None).is_ok()
}

pub fn read_pids(path: &Path) -> Vec<u32> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| line.trim().parse().unwrap())
        .collect()
}

pub async fn wait_until_gone(pids: &[u32]) {
    timeout(EVENT_TIMEOUT, async {
        while pids.iter().any(|pid| process_exists(*pid)) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("processes are still alive");
}

pub struct Session {
    pub handle: SupervisorHandle,
    pub history: Vec<Event>,
    pub dir: tempfile::TempDir,
}

impl Session {
    pub fn start(build: impl FnOnce(&Path) -> Vec<LaunchSpec>) -> Self {
        Self::start_with(options(), build)
    }

    pub fn start_with(
        options: SupervisorOptions,
        build: impl FnOnce(&Path) -> Vec<LaunchSpec>,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let specs = build(dir.path());
        let handle = Supervisor::spawn(specs, options).unwrap();
        Self {
            handle,
            history: Vec::new(),
            dir,
        }
    }

    pub fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    pub fn send(&self, command: Command) {
        self.handle.send(command).unwrap();
    }

    pub async fn until(&mut self, mut matches: impl FnMut(&Event) -> bool) -> Event {
        let history = &mut self.history;
        let handle = &mut self.handle;
        timeout(EVENT_TIMEOUT, async {
            loop {
                let event = handle.recv().await.expect("supervisor ended");
                history.push(event.clone());
                if matches(&event) {
                    return event;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out; history: {:#?}", self.history))
    }

    pub async fn until_state(
        &mut self,
        raw_id: &str,
        matches: impl Fn(&ServiceState) -> bool,
    ) -> ServiceState {
        let wanted = id(raw_id);
        match self
            .until(|event| {
                matches!(event, Event::State { service, state } if *service == wanted && matches(state))
            })
            .await
        {
            Event::State { state, .. } => state,
            _ => unreachable!(),
        }
    }

    pub async fn until_running(&mut self, raw_id: &str) -> u32 {
        match self
            .until_state(raw_id, |state| {
                matches!(state, ServiceState::Running { .. })
            })
            .await
        {
            ServiceState::Running { pid } => pid,
            _ => unreachable!(),
        }
    }

    pub async fn until_output(&mut self, raw_id: &str, needle: &str) {
        let wanted = id(raw_id);
        let mut seen = self.output_of(raw_id, OutputSource::Service);
        if seen.contains(needle) {
            return;
        }
        self.until(|event| match event {
            Event::Output { service, bytes, .. } if *service == wanted => {
                seen.push_str(&String::from_utf8_lossy(bytes));
                seen.contains(needle)
            }
            _ => false,
        })
        .await;
    }

    pub async fn fence(&mut self) {
        self.send(Command::Stop(id("fence-probe")));
        self.until(|event| matches!(event, Event::Rejected { .. }))
            .await;
    }

    pub fn current_pid(&self, raw_id: &str) -> u32 {
        let wanted = id(raw_id);
        let snapshot = self.handle.snapshot();
        let status = snapshot.iter().find(|status| status.id == wanted).unwrap();
        match status.state {
            ServiceState::Running { pid } | ServiceState::Ready { pid } => pid,
            ref other => panic!("{raw_id} is not running: {other:?}"),
        }
    }

    pub fn states_of(&self, raw_id: &str) -> Vec<ServiceState> {
        let wanted = id(raw_id);
        self.history
            .iter()
            .filter_map(|event| match event {
                Event::State { service, state } if *service == wanted => Some(state.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn output_of(&self, raw_id: &str, source: OutputSource) -> String {
        let wanted = id(raw_id);
        self.history
            .iter()
            .filter_map(|event| match event {
                Event::Output {
                    service,
                    source: event_source,
                    bytes,
                } if *service == wanted && *event_source == source => {
                    Some(String::from_utf8_lossy(bytes).into_owned())
                }
                _ => None,
            })
            .collect()
    }

    pub fn position(&self, raw_id: &str, matches: impl Fn(&ServiceState) -> bool) -> usize {
        let wanted = id(raw_id);
        self.history
            .iter()
            .position(|event| {
                matches!(event, Event::State { service, state } if *service == wanted && matches(state))
            })
            .unwrap_or_else(|| panic!("state not found; history: {:#?}", self.history))
    }

    pub fn position_after(
        &self,
        raw_id: &str,
        after: usize,
        matches: impl Fn(&ServiceState) -> bool,
    ) -> Option<usize> {
        let wanted = id(raw_id);
        self.history
            .iter()
            .enumerate()
            .skip(after + 1)
            .find_map(|(position, event)| {
                matches!(event, Event::State { service, state } if *service == wanted && matches(state))
                    .then_some(position)
            })
    }

    pub async fn shutdown(&mut self) {
        self.send(Command::Shutdown);
        self.until(|event| matches!(event, Event::ShutdownComplete))
            .await;
    }
}
