use std::ffi::OsString;
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use portable_pty::{CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use tokio::time::{Instant, sleep};

use crate::LaunchSpec;

const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(20);
const KILL_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const READ_BUFFER_SIZE: usize = 8192;

pub(crate) type ExitCode = Option<i32>;

#[derive(Default)]
struct Lifecycle {
    leader_exit: OnceLock<ExitCode>,
    output_drained: AtomicBool,
    group_gone: AtomicBool,
}

impl Lifecycle {
    fn is_quiet(&self, group: Pid) -> bool {
        self.leader_exit.get().is_some()
            && self.output_drained.load(Ordering::Acquire)
            && !group_alive(group)
    }
}

pub(crate) struct ManagedProcess {
    pid: u32,
    group: Pid,
    master: Box<dyn MasterPty + Send>,
    input: mpsc::Sender<Vec<u8>>,
    lifecycle: Arc<Lifecycle>,
}

impl ManagedProcess {
    pub fn spawn(
        spec: &LaunchSpec,
        command: &str,
        size: PtySize,
        on_output: impl Fn(Vec<u8>) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(size)
            .map_err(|error| error.to_string())?;
        let child = pair
            .slave
            .spawn_command(shell_command(spec, command)?)
            .map_err(|error| error.to_string())?;
        drop(pair.slave);
        let pid = child
            .process_id()
            .ok_or_else(|| "process id is unavailable".to_owned())?;
        let group = Pid::from_raw(i32::try_from(pid).map_err(|error| error.to_string())?);
        let lifecycle = Arc::new(Lifecycle::default());
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| error.to_string())?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| error.to_string())?;
        forward_output(reader, Arc::clone(&lifecycle), on_output);
        wait_for_leader(child, Arc::clone(&lifecycle), on_exit);
        Ok(Self {
            pid,
            group,
            master: pair.master,
            input: forward_input(writer),
            lifecycle,
        })
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(bytes);
    }

    pub fn resize(&self, size: PtySize) {
        let _ = self.master.resize(size);
    }

    pub fn terminator(&self) -> GroupTerminator {
        GroupTerminator {
            group: self.group,
            lifecycle: Arc::clone(&self.lifecycle),
        }
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        if !self.lifecycle.group_gone.load(Ordering::Acquire) {
            let _ = killpg(self.group, Signal::SIGKILL);
        }
    }
}

pub(crate) struct GroupTerminator {
    group: Pid,
    lifecycle: Arc<Lifecycle>,
}

impl GroupTerminator {
    pub async fn terminate(self, grace_period: Duration) -> ExitCode {
        let _ = killpg(self.group, Signal::SIGTERM);
        if !self.settle_within(grace_period).await {
            let _ = killpg(self.group, Signal::SIGKILL);
            self.settle_within(KILL_SETTLE_TIMEOUT).await;
        }
        self.lifecycle.group_gone.store(true, Ordering::Release);
        self.lifecycle.leader_exit.get().copied().flatten()
    }

    async fn settle_within(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.lifecycle.is_quiet(self.group) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            sleep(GROUP_POLL_INTERVAL.min(deadline - now)).await;
        }
    }
}

fn group_alive(group: Pid) -> bool {
    killpg(group, None) != Err(Errno::ESRCH)
}

fn shell_command(spec: &LaunchSpec, command: &str) -> Result<CommandBuilder, String> {
    let mut builder = CommandBuilder::new("/bin/sh");
    builder.arg("-c");
    builder.arg(command);
    builder.cwd(&spec.cwd);
    for (key, value) in &spec.env {
        builder.env(key, value);
    }
    builder.env("PATH", search_path(spec)?);
    Ok(builder)
}

fn search_path(spec: &LaunchSpec) -> Result<OsString, String> {
    let inherited = spec
        .env
        .get("PATH")
        .map(OsString::from)
        .or_else(|| std::env::var_os("PATH"))
        .unwrap_or_default();
    let entries = spec
        .path_prefix
        .iter()
        .cloned()
        .chain(std::env::split_paths(&inherited));
    std::env::join_paths(entries).map_err(|error| error.to_string())
}

fn forward_output(
    mut reader: Box<dyn Read + Send>,
    lifecycle: Arc<Lifecycle>,
    on_output: impl Fn(Vec<u8>) + Send + 'static,
) {
    thread::spawn(move || {
        let mut buffer = [0_u8; READ_BUFFER_SIZE];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => on_output(buffer[..read].to_vec()),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        lifecycle.output_drained.store(true, Ordering::Release);
    });
}

fn forward_input(mut writer: Box<dyn Write + Send>) -> mpsc::Sender<Vec<u8>> {
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    thread::spawn(move || {
        for bytes in receiver {
            if writer
                .write_all(&bytes)
                .and_then(|()| writer.flush())
                .is_err()
            {
                break;
            }
        }
    });
    sender
}

fn wait_for_leader(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    lifecycle: Arc<Lifecycle>,
    on_exit: impl FnOnce() + Send + 'static,
) {
    thread::spawn(move || {
        let code = child.wait().ok().and_then(|status| exit_code(&status));
        let _ = lifecycle.leader_exit.set(code);
        on_exit();
    });
}

fn exit_code(status: &ExitStatus) -> ExitCode {
    match status.signal() {
        Some(_) => None,
        None => i32::try_from(status.exit_code()).ok(),
    }
}
