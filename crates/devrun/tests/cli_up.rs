#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use support::Sandbox;

const RUN_TIMEOUT: Duration = Duration::from_secs(30);

fn service(name: &str, path: &str, command: &str, extra: &str) -> String {
    format!(
        "[services.{name}]\nruntime = \"custom\"\npath = \"{path}\"\ncommand = \"{command}\"\n{extra}\n"
    )
}

fn sandbox_with(services: &[String]) -> Sandbox {
    let sandbox = Sandbox::new();
    let mut body = String::from("schema_version = 1\nroot = \"{root}\"\n\n");
    for definition in services {
        body.push_str(definition);
        body.push('\n');
    }
    sandbox.write_config(&body);
    sandbox
}

#[test]
fn up_without_targets_and_without_a_terminal_explains_how_to_pass_targets() {
    let sandbox = sandbox_with(&[service("web", "apps/web", "echo hi", "")]);
    sandbox
        .devrun()
        .arg("up")
        .timeout(RUN_TIMEOUT)
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            "error: no targets given and no terminal for the picker; pass services, aliases or @profiles\n",
        );
}

#[test]
fn up_refuses_to_start_anything_when_a_planned_port_is_taken() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sandbox = sandbox_with(&[
        service("db", "apps/web", "touch db-started", ""),
        service(
            "api",
            "apps/web",
            "touch api-started",
            &format!("ports = [{port}]\nneeds = [\"db\"]\nprepare = [\"touch api-prepared\"]"),
        ),
    ]);

    sandbox
        .devrun()
        .args(["up", "api", "--no-tui"])
        .timeout(RUN_TIMEOUT)
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!(
            "error: port {port} is already in use (needed by api)\n"
        ));

    thread::sleep(Duration::from_millis(200));
    for marker in ["db-started", "api-started", "api-prepared"] {
        assert!(
            !sandbox.root().join("apps/web").join(marker).exists(),
            "{marker}"
        );
    }
}

#[test]
fn headless_streams_prefixed_lines_in_needs_order_and_exits_zero_when_all_succeed() {
    let sandbox = sandbox_with(&[
        service("db", "apps/web", "sleep 0.2; printf 'db-line\\\\n'", ""),
        service(
            "web",
            "apps/web",
            "sleep 1.2; printf '\\\\033[32mweb-line\\\\033[0m\\\\n'",
            "needs = [\"db\"]",
        ),
    ]);

    sandbox
        .devrun()
        .args(["up", "--no-tui", "web"])
        .timeout(RUN_TIMEOUT)
        .assert()
        .success()
        .stderr("")
        .stdout(
            "db  · starting
db  · running
web · starting
web · running
db  | db-line
db  · exited with code 0
web | web-line
web · exited with code 0
2 services finished cleanly
",
        );
}

#[test]
fn headless_is_used_automatically_when_stdout_is_not_a_terminal() {
    let sandbox = sandbox_with(&[service("web", "apps/web", "sleep 0.2; echo web-line", "")]);

    sandbox
        .devrun()
        .args(["up", "web"])
        .timeout(RUN_TIMEOUT)
        .assert()
        .success()
        .stderr("")
        .stdout(
            "web · starting
web · running
web | web-line
web · exited with code 0
1 service finished cleanly
",
        );
}

#[test]
fn headless_exits_one_and_names_services_that_did_not_finish_cleanly() {
    let sandbox = sandbox_with(&[service(
        "api",
        "apps/web",
        "sleep 0.2; echo boom; exit 3",
        "",
    )]);

    sandbox
        .devrun()
        .args(["up", "api", "--no-tui"])
        .timeout(RUN_TIMEOUT)
        .assert()
        .code(1)
        .stdout(
            "api · starting
api · running
api | boom
api · exited with code 3
",
        )
        .stderr("error: not every service finished cleanly: api\n");
}

#[test]
fn headless_reports_prepare_failures_and_blocked_dependents() {
    let sandbox = sandbox_with(&[
        service(
            "api",
            "apps/web",
            "echo never-runs",
            "prepare = [\"echo installing; exit 4\"]",
        ),
        service("web", "apps/web", "echo never-runs", "needs = [\"api\"]"),
    ]);

    sandbox
        .devrun()
        .args(["up", "web", "--no-tui"])
        .timeout(RUN_TIMEOUT)
        .assert()
        .code(1)
        .stdout(
            "api · preparing 1/1
api | installing
api · failed: prepare step 1/1 failed with code 4
web · failed: needs api, which failed
",
        )
        .stderr("error: not every service finished cleanly: api, web\n");
}

#[test]
fn up_warns_about_needs_that_were_excluded_and_runs_the_rest() {
    let sandbox = sandbox_with(&[
        service("db", "apps/web", "touch db-started", ""),
        service(
            "web",
            "apps/web",
            "sleep 0.2; echo web-line",
            "needs = [\"db\"]",
        ),
    ]);

    sandbox
        .devrun()
        .args(["up", "web", "-db", "--no-tui"])
        .timeout(RUN_TIMEOUT)
        .assert()
        .success()
        .stderr("warning: web needs db, which was excluded\n")
        .stdout(
            "web · starting
web · running
web | web-line
web · exited with code 0
1 service finished cleanly
",
        );
    assert!(!sandbox.root().join("apps/web/db-started").exists());
}

#[test]
fn python_services_run_through_the_project_venv_when_it_exists() {
    let sandbox = sandbox_with(&[
        "[services.kg]\nruntime = \"python\"\npath = \"apps/kg\"\ncommand = \"poetry run greet\"\nprepare = [\"uv run greet prepared\"]\n"
            .to_owned(),
    ]);
    let tool = sandbox.root().join("apps/kg/.venv/bin/greet");
    sandbox.write(
        "apps/kg/.venv/bin/greet",
        "#!/bin/sh\necho \"venv-${1:-service}\"\n",
    );
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();

    sandbox
        .devrun()
        .args(["up", "kg", "--no-tui"])
        .timeout(RUN_TIMEOUT)
        .assert()
        .success()
        .stderr("")
        .stdout(
            "kg · preparing 1/1
kg | venv-prepared
kg · starting
kg · running
kg | venv-service
kg · exited with code 0
1 service finished cleanly
",
        );
}

fn spawn_headless(sandbox: &Sandbox, target: &str) -> Child {
    std::process::Command::new(assert_cmd::cargo::cargo_bin!("devrun"))
        .args(["up", target, "--no-tui"])
        .current_dir(sandbox.root())
        .env("DEVRUN_CONFIG_DIR", sandbox.config_dir())
        .env_remove("XDG_CONFIG_HOME")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

fn stream_lines(child: &mut Child) -> mpsc::Receiver<String> {
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    receiver
}

fn wait_with_deadline(child: &mut Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + RUN_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("devrun did not exit after SIGINT");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn process_alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

fn read_pid(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap().trim().to_owned()
}

#[test]
fn interrupt_stops_every_service_and_leaves_no_process_behind() {
    let sandbox = sandbox_with(&[service(
        "web",
        "apps/web",
        "sleep 1000 & echo $! > child.pid; echo web-up; wait",
        "",
    )]);
    let mut child = spawn_headless(&sandbox, "web");
    let lines = stream_lines(&mut child);
    let mut seen = Vec::new();
    while !seen.iter().any(|line: &String| line == "web | web-up") {
        let Ok(line) = lines.recv_timeout(RUN_TIMEOUT) else {
            let _ = child.kill();
            panic!("service never started: {seen:?}");
        };
        seen.push(line);
    }
    let grandchild = read_pid(&sandbox.root().join("apps/web/child.pid"));

    let interrupted = std::process::Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    let status = wait_with_deadline(&mut child);
    seen.extend(lines.iter());

    assert!(interrupted.success());
    assert_eq!(status.code(), Some(0), "{seen:?}");
    assert_eq!(
        seen[seen.len() - 3..],
        ["web · stopping", "web · stopped", "stopped 1 service"]
    );
    assert!(!process_alive(&grandchild), "sleep {grandchild} survived");
}
