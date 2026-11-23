#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;

use common::{Session, id, options, spec};
use devrun_supervisor::{
    Command, Event, LaunchSpec, OutputSource, ServiceState, ServiceStatus, Supervisor,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

fn spawn_error(specs: Vec<LaunchSpec>) -> String {
    match Supervisor::spawn(specs, options()) {
        Ok(_) => panic!("specs should be rejected"),
        Err(error) => error.to_string(),
    }
}

fn needing(raw_id: &str, needs: &[&str]) -> LaunchSpec {
    LaunchSpec {
        needs: needs.iter().map(|need| id(need)).collect(),
        ..spec(raw_id, Path::new("/"), "true")
    }
}

#[tokio::test]
async fn rejects_unknown_need() {
    assert_eq!(
        spawn_error(vec![needing("api", &["db"])]),
        "service `api` needs unknown service `db`"
    );
}

#[tokio::test]
async fn rejects_duplicate_service_id() {
    assert_eq!(
        spawn_error(vec![needing("api", &[]), needing("api", &[])]),
        "service `api` is defined more than once"
    );
}

#[tokio::test]
async fn rejects_dependency_cycle() {
    assert_eq!(
        spawn_error(vec![
            needing("a", &["b"]),
            needing("b", &["c"]),
            needing("c", &["a"]),
        ]),
        "dependency cycle: a -> b -> c -> a"
    );
}

#[test]
fn spawn_outside_a_tokio_runtime_is_an_error() {
    let error = Supervisor::spawn(vec![needing("api", &[])], options()).err();

    assert_eq!(
        error.map(|error| error.to_string()),
        Some("supervisor must be spawned inside a tokio runtime".to_owned())
    );
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(values: &[T]) {
    for value in values {
        let json = serde_json::to_string(value).unwrap();
        let decoded: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&decoded, value, "{json}");
    }
}

fn every_state() -> Vec<ServiceState> {
    vec![
        ServiceState::Waiting { on: vec![id("db")] },
        ServiceState::Preparing { step: 1, total: 2 },
        ServiceState::Starting,
        ServiceState::Running { pid: 42 },
        ServiceState::Ready { pid: 42 },
        ServiceState::Stopping,
        ServiceState::Stopped,
        ServiceState::Exited { code: Some(3) },
        ServiceState::Exited { code: None },
        ServiceState::Failed {
            reason: "needs db, which failed".to_owned(),
        },
    ]
}

#[test]
fn commands_round_trip_through_json() {
    round_trip(&[
        Command::Start(id("api")),
        Command::Stop(id("api")),
        Command::Restart(id("api")),
        Command::Resize {
            rows: 40,
            cols: 100,
        },
        Command::Input {
            service: id("api"),
            bytes: b"q\n".to_vec(),
        },
        Command::Shutdown,
    ]);
}

#[test]
fn events_round_trip_through_json() {
    let mut events: Vec<Event> = every_state()
        .into_iter()
        .map(|state| Event::State {
            service: id("api"),
            state,
        })
        .collect();
    events.extend([
        Event::Output {
            service: id("api"),
            source: OutputSource::Prepare,
            bytes: b"\x1b[32mok\x1b[0m".to_vec(),
        },
        Event::Output {
            service: id("api"),
            source: OutputSource::Service,
            bytes: vec![0, 255, 13, 10],
        },
        Event::Rejected {
            reason: "unknown service `ghost`".to_owned(),
        },
        Event::ShutdownComplete,
    ]);
    round_trip(&events);
}

#[test]
fn states_and_statuses_round_trip_through_json() {
    round_trip(&every_state());
    round_trip(&[ServiceStatus {
        id: id("api"),
        state: ServiceState::Ready { pid: 7 },
    }]);
}

#[test]
fn service_id_serializes_as_a_plain_string() {
    assert_eq!(serde_json::to_string(&id("api")).unwrap(), "\"api\"");
}

#[tokio::test]
async fn commands_for_unknown_services_are_rejected_without_stopping_the_supervisor() {
    let mut session = Session::start(|dir| vec![spec("web", dir, "sleep 1000")]);
    session.until_running("web").await;

    session.send(Command::Restart(id("ghost")));
    let rejected = session
        .until(|event| matches!(event, Event::Rejected { .. }))
        .await;

    assert_eq!(
        rejected,
        Event::Rejected {
            reason: "unknown service `ghost`".to_owned()
        }
    );
    assert!(matches!(
        session.handle.snapshot()[0].state,
        ServiceState::Running { .. }
    ));
    session.shutdown().await;
}

#[tokio::test]
async fn input_and_resize_reach_the_pty() {
    let mut session = Session::start(|dir| {
        vec![spec(
            "web",
            dir,
            "read line; echo \"got:$line\"; read _; stty size",
        )]
    });
    session.until_running("web").await;

    session.send(Command::Input {
        service: id("web"),
        bytes: b"abc\n".to_vec(),
    });
    session.until_output("web", "got:abc").await;
    session.send(Command::Resize {
        rows: 40,
        cols: 100,
    });
    session.send(Command::Input {
        service: id("web"),
        bytes: b"\n".to_vec(),
    });
    session.until_output("web", "40 100").await;

    session.shutdown().await;
}

#[tokio::test]
async fn snapshot_lists_every_service_in_spec_order() {
    let mut session =
        Session::start(|dir| vec![spec("web", dir, "sleep 1000"), spec("done", dir, "exit 0")]);
    session.until_running("web").await;
    session
        .until_state("done", |state| matches!(state, ServiceState::Exited { .. }))
        .await;

    let snapshot = session.handle.snapshot();

    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[0].id, id("web"));
    assert!(matches!(snapshot[0].state, ServiceState::Running { .. }));
    assert_eq!(
        snapshot[1],
        ServiceStatus {
            id: id("done"),
            state: ServiceState::Exited { code: Some(0) }
        }
    );
    session.shutdown().await;
}
