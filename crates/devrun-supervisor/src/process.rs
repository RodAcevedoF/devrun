use std::ffi::{OsStr, OsString};
use std::fmt::Display;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};
use std::thread;
use std::time::Duration;

use nix::errno::Errno;
use nix::sys::signal::{Signal, killpg};
use nix::unistd::Pid;
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use tokio::time::{Instant, sleep};

use crate::LaunchSpec;

const GROUP_POLL_INTERVAL: Duration = Duration::from_millis(20);
const KILL_SETTLE_TIMEOUT: Duration = Duration::from_secs(2);
const READ_BUFFER_SIZE: usize = 8192;
const LOW_PRIORITY_NICENESS: &str = "10";
const NICE: &str = "nice";
const EXECUTABLE_BITS: u32 = 0o111;
const SHELL: &str = "/bin/sh";

pub(crate) type ExitCode = Option<i32>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    Normal,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Launcher {
    Niced(PathBuf),
    Direct,
}

fn low_priority_launcher(search_path: Option<&OsStr>) -> Launcher {
    search_path
        .into_iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(NICE))
        .find(|candidate| is_executable(candidate))
        .map_or(Launcher::Direct, Launcher::Niced)
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| {
        metadata.is_file() && metadata.permissions().mode() & EXECUTABLE_BITS != 0
    })
}

fn supervisor_low_priority_launcher() -> &'static Launcher {
    static LAUNCHER: OnceLock<Launcher> = OnceLock::new();
    LAUNCHER.get_or_init(|| low_priority_launcher(std::env::var_os("PATH").as_deref()))
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitWording {
    Exited,
    Failed,
}

pub(crate) fn describe_exit(code: ExitCode, wording: ExitWording) -> String {
    let verb = match wording {
        ExitWording::Exited => "exited",
        ExitWording::Failed => "failed",
    };
    match code {
        Some(code) => format!("{verb} with code {code}"),
        None => "was terminated by a signal".to_owned(),
    }
}

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
        priority: Priority,
        size: PtySize,
        on_output: impl Fn(Vec<u8>) + Send + 'static,
        on_exit: impl FnOnce() + Send + 'static,
    ) -> Result<Self, String> {
        let pair = native_pty_system().openpty(size).map_err(single_line)?;
        let mut child = pair
            .slave
            .spawn_command(shell_command(spec, command, priority)?)
            .map_err(single_line)?;
        drop(pair.slave);
        let attached = attach(child.as_ref(), pair.master.as_ref());
        let (pid, group, reader, writer) = match attached {
            Ok(parts) => parts,
            Err(error) => {
                let _ = child.kill();
                return Err(error);
            }
        };
        let lifecycle = Arc::new(Lifecycle::default());
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

type AttachedPty = (u32, Pid, Box<dyn Read + Send>, Box<dyn Write + Send>);

fn attach(
    child: &(dyn Child + Send + Sync),
    master: &dyn MasterPty,
) -> Result<AttachedPty, String> {
    let pid = child
        .process_id()
        .ok_or_else(|| "process id is unavailable".to_owned())?;
    let group = Pid::from_raw(i32::try_from(pid).map_err(single_line)?);
    let reader = master.try_clone_reader().map_err(single_line)?;
    let writer = master.take_writer().map_err(single_line)?;
    Ok((pid, group, reader, writer))
}

fn group_alive(group: Pid) -> bool {
    killpg(group, None) != Err(Errno::ESRCH)
}

fn shell_command(
    spec: &LaunchSpec,
    command: &str,
    priority: Priority,
) -> Result<CommandBuilder, String> {
    if !spec.cwd.is_dir() {
        return Err(format!(
            "working directory {} does not exist",
            spec.cwd.display()
        ));
    }
    let launcher = match priority {
        Priority::Normal => &Launcher::Direct,
        Priority::Low => supervisor_low_priority_launcher(),
    };
    let mut builder = match launcher {
        Launcher::Direct => CommandBuilder::new(SHELL),
        Launcher::Niced(nice) => {
            let mut niced = CommandBuilder::new(nice);
            niced.args(["-n", LOW_PRIORITY_NICENESS, SHELL]);
            niced
        }
    };
    builder.arg("-c");
    builder.arg(command);
    builder.cwd(&spec.cwd);
    for (key, value) in &spec.env {
        builder.env(key, value);
    }
    builder.env("PATH", search_path(spec)?);
    Ok(builder)
}

fn single_line(error: impl Display) -> String {
    error
        .to_string()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
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
    std::env::join_paths(entries).map_err(single_line)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_multi_line_failure_reason_folds_into_one_line() {
        assert_eq!(
            single_line("Unable to spawn nice because:\n  No viable candidates found.\n\nnone\n"),
            "Unable to spawn nice because: No viable candidates found. none"
        );
    }

    #[test]
    fn a_search_path_without_nice_falls_back_to_normal_priority() {
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            low_priority_launcher(Some(empty.path().as_os_str())),
            Launcher::Direct
        );
        assert_eq!(low_priority_launcher(None), Launcher::Direct);
    }

    #[test]
    fn the_first_executable_nice_on_the_search_path_launches_low_priority_work() {
        let plain = tempfile::tempdir().unwrap();
        let executable = tempfile::tempdir().unwrap();
        std::fs::write(plain.path().join("nice"), "").unwrap();
        let nice = executable.path().join("nice");
        std::fs::write(&nice, "").unwrap();
        std::fs::set_permissions(&nice, std::fs::Permissions::from_mode(0o755)).unwrap();
        let search_path = std::env::join_paths([plain.path(), executable.path()]).unwrap();

        assert_eq!(
            low_priority_launcher(Some(&search_path)),
            Launcher::Niced(nice)
        );
    }

    #[test]
    fn an_exit_reads_as_its_code_or_as_a_signal() {
        assert_eq!(
            describe_exit(Some(3), ExitWording::Exited),
            "exited with code 3"
        );
        assert_eq!(
            describe_exit(Some(4), ExitWording::Failed),
            "failed with code 4"
        );
        for wording in [ExitWording::Exited, ExitWording::Failed] {
            assert_eq!(describe_exit(None, wording), "was terminated by a signal");
        }
    }
}
