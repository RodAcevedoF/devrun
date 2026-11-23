#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;

use common::{Session, free_port, id, spec};
use devrun_supervisor::{Command, LaunchSpec, OutputSource, ServiceState};

#[tokio::test]
async fn service_output_is_forwarded_and_exit_code_reported() {
    let mut session = Session::start(|dir| vec![spec("web", dir, "echo hello-from-pty; exit 3")]);

    let state = session
        .until_state("web", |state| matches!(state, ServiceState::Exited { .. }))
        .await;

    assert_eq!(state, ServiceState::Exited { code: Some(3) });
    assert!(
        session
            .output_of("web", OutputSource::Service)
            .contains("hello-from-pty")
    );
    session.shutdown().await;
}

#[tokio::test]
async fn command_runs_in_cwd_with_env_and_path_prefix() {
    let mut session = Session::start(|dir| {
        let bin = dir.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let tool = bin.join("hello-tool");
        std::fs::write(&tool, "#!/bin/sh\necho tool-on-path\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        vec![LaunchSpec {
            env: BTreeMap::from([("GREETING".to_owned(), "hola".to_owned())]),
            path_prefix: vec![bin],
            ..spec(
                "web",
                dir,
                "hello-tool; echo \"greeting=$GREETING\"; pwd -P",
            )
        }]
    });
    let expected_dir = session.dir.path().canonicalize().unwrap();

    session
        .until_state("web", |state| matches!(state, ServiceState::Exited { .. }))
        .await;

    let output = session.output_of("web", OutputSource::Service);
    assert!(output.contains("tool-on-path"), "{output}");
    assert!(output.contains("greeting=hola"), "{output}");
    assert!(output.contains(expected_dir.to_str().unwrap()), "{output}");
    session.shutdown().await;
}

#[tokio::test]
async fn dependent_waits_until_need_is_ready_then_starts() {
    let port = free_port();
    let mut session = Session::start(|dir| {
        vec![
            LaunchSpec {
                needs: vec![id("db")],
                ..spec("api", dir, "sleep 1000")
            },
            LaunchSpec {
                ports: vec![port],
                ..spec("db", dir, "sleep 1000")
            },
        ]
    });

    session.until_running("db").await;
    session.fence().await;
    assert_eq!(
        session.handle.snapshot()[0].state,
        ServiceState::Waiting { on: vec![id("db")] }
    );
    assert!(session.states_of("api").is_empty());

    let _listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
    session
        .until_state("db", |state| matches!(state, ServiceState::Ready { .. }))
        .await;
    session.until_running("api").await;

    let ready = session.position("db", |state| matches!(state, ServiceState::Ready { .. }));
    let started = session.position("api", |state| *state == ServiceState::Starting);
    assert!(ready < started);
    session.shutdown().await;
}

#[tokio::test]
async fn need_without_ports_counts_as_ready_once_running() {
    let mut session = Session::start(|dir| {
        vec![
            spec("base", dir, "sleep 1000"),
            LaunchSpec {
                needs: vec![id("base")],
                ..spec("worker", dir, "sleep 1000")
            },
        ]
    });

    session.until_running("worker").await;

    let base = session.position("base", |state| {
        matches!(state, ServiceState::Running { .. })
    });
    let worker = session.position("worker", |state| *state == ServiceState::Starting);
    assert!(base < worker);
    session.shutdown().await;
}

#[tokio::test]
async fn dependents_fail_when_need_exits_or_fails() {
    let unbound_port = free_port();
    let mut session = Session::start(|dir| {
        vec![
            LaunchSpec {
                ports: vec![unbound_port],
                ..spec("db", dir, "exit 4")
            },
            LaunchSpec {
                needs: vec![id("db")],
                ..spec("api", dir, "sleep 1000")
            },
            LaunchSpec {
                needs: vec![id("api")],
                ..spec("web", dir, "sleep 1000")
            },
            LaunchSpec {
                prepare: vec!["exit 1".to_owned()],
                ..spec("cache", dir, "sleep 1000")
            },
            LaunchSpec {
                needs: vec![id("cache")],
                ..spec("jobs", dir, "sleep 1000")
            },
        ]
    });

    let api = session
        .until_state("api", |state| matches!(state, ServiceState::Failed { .. }))
        .await;
    let web = session
        .until_state("web", |state| matches!(state, ServiceState::Failed { .. }))
        .await;
    let jobs = session
        .until_state("jobs", |state| matches!(state, ServiceState::Failed { .. }))
        .await;

    assert_eq!(
        api,
        ServiceState::Failed {
            reason: "needs db, which exited with code 4".to_owned()
        }
    );
    assert_eq!(
        web,
        ServiceState::Failed {
            reason: "needs api, which failed".to_owned()
        }
    );
    assert_eq!(
        jobs,
        ServiceState::Failed {
            reason: "needs cache, which failed".to_owned()
        }
    );
    assert!(
        session
            .states_of("api")
            .iter()
            .all(|state| !matches!(state, ServiceState::Starting))
    );
    session.shutdown().await;
}

#[tokio::test]
async fn prepare_steps_run_in_order_before_start() {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec!["echo prep-one".to_owned(), "echo prep-two".to_owned()],
            ..spec("web", dir, "echo main-started; sleep 1000")
        }]
    });

    session.until_running("web").await;
    session.until_output("web", "main-started").await;

    let states = session.states_of("web");
    assert_eq!(
        states[..3],
        [
            ServiceState::Preparing { step: 1, total: 2 },
            ServiceState::Preparing { step: 2, total: 2 },
            ServiceState::Starting,
        ]
    );
    assert!(matches!(states[3], ServiceState::Running { .. }));
    let prepare_output = session.output_of("web", OutputSource::Prepare);
    let one = prepare_output.find("prep-one").unwrap();
    let two = prepare_output.find("prep-two").unwrap();
    assert!(one < two);
    assert!(
        !session
            .output_of("web", OutputSource::Service)
            .contains("prep-")
    );
    session.shutdown().await;
}

#[tokio::test]
async fn prepare_failure_blocks_start_with_exact_reason() {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec!["true".to_owned(), "exit 7".to_owned()],
            ..spec("web", dir, "echo main-started; sleep 1000")
        }]
    });

    let state = session
        .until_state("web", |state| matches!(state, ServiceState::Failed { .. }))
        .await;
    session.fence().await;

    assert_eq!(
        state,
        ServiceState::Failed {
            reason: "prepare step 2/2 failed with code 7".to_owned()
        }
    );
    assert!(
        session
            .states_of("web")
            .iter()
            .all(|state| !matches!(state, ServiceState::Starting))
    );
    session.shutdown().await;
}

#[tokio::test]
async fn restart_skips_prepare_and_relaunches_service() {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec!["echo prep-ran".to_owned()],
            ..spec("web", dir, "echo main-run; sleep 1000")
        }]
    });
    let first_pid = session.until_running("web").await;

    session.send(Command::Restart(id("web")));
    session
        .until_state("web", |state| *state == ServiceState::Stopped)
        .await;
    let second_pid = session.until_running("web").await;
    session.until_output("web", "main-run").await;

    assert_ne!(first_pid, second_pid);
    let preparing = session
        .states_of("web")
        .iter()
        .filter(|state| matches!(state, ServiceState::Preparing { .. }))
        .count();
    assert_eq!(preparing, 1);
    assert_eq!(
        session
            .output_of("web", OutputSource::Prepare)
            .matches("prep-ran")
            .count(),
        1
    );
    session.shutdown().await;
}
