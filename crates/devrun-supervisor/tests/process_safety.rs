#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Session, free_port, id, options, process_exists, read_pids, spec, wait_until_gone};
use devrun_supervisor::{
    Command, Event, LaunchSpec, OutputSource, ServiceState, SupervisorOptions,
};

const TWO_CHILDREN: &str = "trap '' HUP; sleep 1000 & echo $! >> \"$PIDS\"; sleep 1000 & echo $! >> \"$PIDS\"; echo children-started; wait";

fn with_pid_file(dir: &Path, raw_id: &str, command: &str) -> LaunchSpec {
    let pid_file = dir.join(format!("{raw_id}.pids"));
    LaunchSpec {
        env: BTreeMap::from([("PIDS".to_owned(), pid_file.display().to_string())]),
        ..spec(raw_id, dir, command)
    }
}

fn ps_field(pid: u32, field: &str) -> Option<i32> {
    let out = std::process::Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

async fn group_pids(session: &mut Session, raw_id: &str) -> Vec<u32> {
    session.until_output(raw_id, "children-started").await;
    let leader = session.current_pid(raw_id);
    let mut pids = read_pids(&session.path(&format!("{raw_id}.pids")));
    assert_eq!(pids.len(), 2);
    pids.push(leader);
    pids
}

#[tokio::test]
async fn child_is_own_group_and_session_leader() {
    let mut session = Session::start(|dir| vec![with_pid_file(dir, "web", TWO_CHILDREN)]);
    let pids = group_pids(&mut session, "web").await;
    let leader = *pids.last().unwrap();
    let own_pgid = nix::unistd::getpgrp().as_raw();
    assert_eq!(
        ps_field(leader, "pgid"),
        Some(i32::try_from(leader).unwrap())
    );
    assert!(ps_field(leader, "sess").is_some());
    for child in &pids[..2] {
        assert_eq!(
            ps_field(*child, "pgid"),
            Some(i32::try_from(leader).unwrap())
        );
    }
    assert_ne!(own_pgid, i32::try_from(leader).unwrap());
    session.shutdown().await;
    assert!(pids.iter().all(|pid| !process_exists(*pid)), "{pids:?}");
}

#[tokio::test]
async fn restart_kills_grandchildren_of_previous_run() {
    let mut session = Session::start(|dir| vec![with_pid_file(dir, "web", TWO_CHILDREN)]);
    let first = group_pids(&mut session, "web").await;
    std::fs::remove_file(session.path("web.pids")).unwrap();
    session.send(Command::Restart(id("web")));
    session
        .until_state("web", |s| *s == ServiceState::Stopped)
        .await;
    let second_pid = session.until_running("web").await;
    assert!(
        first.iter().all(|pid| !process_exists(*pid)),
        "survivors {first:?}"
    );
    assert!(!first.contains(&second_pid));
    let deadline = Instant::now() + Duration::from_secs(5);
    while read_pids(&session.path("web.pids")).len() < 2 {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut second = read_pids(&session.path("web.pids"));
    second.push(second_pid);
    session.shutdown().await;
    assert!(
        second.iter().all(|pid| !process_exists(*pid)),
        "survivors {second:?}"
    );
}

#[tokio::test]
async fn shutdown_escalates_for_trapped_term() {
    let stubborn = "trap '' TERM HUP; sleep 1000 & echo $! >> \"$PIDS\"; sleep 1000 & echo $! >> \"$PIDS\"; echo children-started; while :; do sleep 1; done";
    let mut session = Session::start_with(
        SupervisorOptions {
            grace_period: Duration::from_millis(400),
            ..options()
        },
        |dir| vec![with_pid_file(dir, "web", stubborn)],
    );
    let pids = group_pids(&mut session, "web").await;
    let started = Instant::now();
    session.shutdown().await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert!(
        pids.iter().all(|pid| !process_exists(*pid)),
        "survivors {pids:?}"
    );
}

#[tokio::test]
async fn restart_escalates_for_trapped_term() {
    let stubborn = "trap '' TERM HUP; sleep 1000 & echo $! >> \"$PIDS\"; sleep 1000 & echo $! >> \"$PIDS\"; echo children-started; while :; do sleep 1; done";
    let mut session = Session::start_with(
        SupervisorOptions {
            grace_period: Duration::from_millis(300),
            ..options()
        },
        |dir| vec![with_pid_file(dir, "web", stubborn)],
    );
    let pids = group_pids(&mut session, "web").await;
    session.send(Command::Restart(id("web")));
    session
        .until_state("web", |s| *s == ServiceState::Stopped)
        .await;
    session.until_running("web").await;
    assert!(
        pids.iter().all(|pid| !process_exists(*pid)),
        "survivors {pids:?}"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn leader_exit_reaps_backgrounded_grandchildren() {
    let mut session = Session::start(|dir| {
        vec![with_pid_file(
            dir,
            "web",
            "trap '' HUP; sleep 1000 & echo $! >> \"$PIDS\"; echo children-started; exit 0",
        )]
    });
    let state = session
        .until_state("web", |s| matches!(s, ServiceState::Exited { .. }))
        .await;
    assert_eq!(state, ServiceState::Exited { code: Some(0) });
    let pids = read_pids(&session.path("web.pids"));
    assert_eq!(pids.len(), 1);
    assert!(
        pids.iter().all(|pid| !process_exists(*pid)),
        "survivors {pids:?}"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn stop_during_prepare_kills_prepare_group_and_start_reruns_prepare() {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec![
                "trap '' HUP; sleep 1000 & echo $! >> \"$PIDS\"; echo children-started; wait"
                    .to_owned(),
            ],
            ..with_pid_file(dir, "web", "echo main; sleep 1000")
        }]
    });
    session
        .until_state("web", |s| matches!(s, ServiceState::Preparing { .. }))
        .await;
    session.until(|e| matches!(e, Event::Output { source: OutputSource::Prepare, bytes, .. } if String::from_utf8_lossy(bytes).contains("children-started"))).await;
    let pids = read_pids(&session.path("web.pids"));
    session.send(Command::Stop(id("web")));
    session
        .until_state("web", |s| *s == ServiceState::Stopped)
        .await;
    assert!(
        pids.iter().all(|pid| !process_exists(*pid)),
        "survivors {pids:?}"
    );
    session.send(Command::Start(id("web")));
    session
        .until_state("web", |s| {
            matches!(s, ServiceState::Preparing { step: 1, total: 1 })
        })
        .await;
    session.shutdown().await;
    let all = read_pids(&session.path("web.pids"));
    wait_until_gone(&all).await;
}

#[tokio::test]
async fn need_killed_by_signal_reason() {
    let port = free_port();
    let mut session = Session::start(|dir| {
        vec![
            LaunchSpec {
                ports: vec![port],
                ..spec("db", dir, "sleep 1000")
            },
            LaunchSpec {
                needs: vec![id("db")],
                ..spec("api", dir, "sleep 1000")
            },
        ]
    });
    let pid = session.until_running("db").await;
    nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(i32::try_from(pid).unwrap()),
        nix::sys::signal::Signal::SIGKILL,
    )
    .unwrap();
    let api = session
        .until_state("api", |s| matches!(s, ServiceState::Failed { .. }))
        .await;
    assert_eq!(
        api,
        ServiceState::Failed {
            reason: "needs db, which was terminated by a signal".to_owned()
        }
    );
    assert_eq!(
        session.handle.snapshot()[0].state,
        ServiceState::Exited { code: None }
    );
    session.shutdown().await;
}

#[tokio::test]
async fn prepare_killed_by_signal_reason() {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec!["kill -9 $$".to_owned()],
            ..spec("web", dir, "sleep 1000")
        }]
    });
    let state = session
        .until_state("web", |s| matches!(s, ServiceState::Failed { .. }))
        .await;
    assert_eq!(
        state,
        ServiceState::Failed {
            reason: "prepare step 1/1 was terminated by a signal".to_owned()
        }
    );
    session.shutdown().await;
}

#[tokio::test]
async fn missing_cwd_fails_instead_of_running_elsewhere() {
    let mut session =
        Session::start(|dir| vec![spec("web", &dir.join("missing"), "echo ran-at=$(pwd -P)")]);
    let state = session
        .until_state("web", |s| {
            matches!(s, ServiceState::Exited { .. } | ServiceState::Failed { .. })
        })
        .await;
    assert!(
        matches!(state, ServiceState::Failed { ref reason } if reason.starts_with("could not start:") && reason.contains("missing")),
        "{state:?}"
    );
    assert!(
        !session
            .output_of("web", OutputSource::Service)
            .contains("ran-at")
    );
    session.shutdown().await;
}

#[tokio::test]
async fn double_restart_and_start_while_stopping() {
    let mut session = Session::start(|dir| vec![with_pid_file(dir, "web", TWO_CHILDREN)]);
    let first = group_pids(&mut session, "web").await;
    session.send(Command::Restart(id("web")));
    session.send(Command::Restart(id("web")));
    session.send(Command::Start(id("web")));
    session
        .until_state("web", |s| *s == ServiceState::Stopped)
        .await;
    session.until_running("web").await;
    session.fence().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    session.fence().await;
    let running = session
        .states_of("web")
        .iter()
        .filter(|s| matches!(s, ServiceState::Running { .. }))
        .count();
    assert_eq!(running, 2, "{:#?}", session.states_of("web"));
    assert!(first.iter().all(|pid| !process_exists(*pid)));
    session.shutdown().await;
    let all = read_pids(&session.path("web.pids"));
    wait_until_gone(&all).await;
}

#[tokio::test]
async fn stop_then_start_does_not_rerun_prepare() {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec!["echo prep-ran".to_owned()],
            ..spec("web", dir, "sleep 1000")
        }]
    });
    session.until_running("web").await;
    session.send(Command::Stop(id("web")));
    session
        .until_state("web", |s| *s == ServiceState::Stopped)
        .await;
    session.send(Command::Start(id("web")));
    session.until_running("web").await;
    assert_eq!(
        session
            .output_of("web", OutputSource::Prepare)
            .matches("prep-ran")
            .count(),
        1
    );
    session.shutdown().await;
}

#[tokio::test]
async fn processes_outside_the_group_are_left_alone_and_do_not_hang_shutdown() {
    let mut session = Session::start_with(
        SupervisorOptions {
            grace_period: Duration::from_millis(200),
            ..options()
        },
        |dir| {
            vec![with_pid_file(
                dir,
                "web",
                "perl -e 'use POSIX; POSIX::setsid(); sleep 997' & echo $! >> \"$PIDS\"; echo children-started; wait",
            )]
        },
    );
    session.until_output("web", "children-started").await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let escaped = read_pids(&session.path("web.pids"));
    let started = Instant::now();
    session.shutdown().await;
    let elapsed = started.elapsed();
    let survived: Vec<u32> = escaped
        .iter()
        .copied()
        .filter(|p| process_exists(*p))
        .collect();
    for pid in &escaped {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(i32::try_from(*pid).unwrap()),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert!(
        !escaped.is_empty() && survived.len() == escaped.len(),
        "{survived:?}"
    );
}

#[test]
fn wire_format_is_externally_tagged_json() {
    let json = |value: serde_json::Value| value.to_string();
    assert_eq!(
        serde_json::to_string(&Command::Start(id("api"))).unwrap(),
        json(serde_json::json!({ "Start": "api" }))
    );
    assert_eq!(
        serde_json::to_string(&Command::Shutdown).unwrap(),
        "\"Shutdown\""
    );
    assert_eq!(
        serde_json::to_string(&Event::State {
            service: id("api"),
            state: ServiceState::Running { pid: 1 }
        })
        .unwrap(),
        json(
            serde_json::json!({ "State": { "service": "api", "state": { "Running": { "pid": 1 } } } })
        )
    );
    assert_eq!(
        serde_json::to_string(&Event::ShutdownComplete).unwrap(),
        "\"ShutdownComplete\""
    );
}
