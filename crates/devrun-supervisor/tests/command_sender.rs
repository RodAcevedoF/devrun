#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::{EVENT_TIMEOUT, Session, id, spec};
use devrun_supervisor::{
    Command, Event, LaunchSpec, RebuildProgress, ServiceState, SupervisorError,
};
use tokio::time::{sleep, timeout};

fn prepared(raw_id: &str, dir: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        prepare: vec!["echo build >> builds.log".to_owned()],
        ..spec(raw_id, dir, "sleep 1000")
    }
}

fn builds(session: &Session) -> usize {
    std::fs::read_to_string(session.path("builds.log"))
        .unwrap_or_default()
        .lines()
        .count()
}

#[tokio::test]
async fn a_cloned_sender_moved_into_another_task_rebuilds_and_stops_services() {
    let mut session = Session::start(|dir| vec![prepared("api", dir)]);
    let first = session.until_running("api").await;
    let commands = session.handle.commands();

    let sender_task = commands.clone();
    tokio::spawn(async move { sender_task.send(Command::Rebuild(id("api"))) })
        .await
        .unwrap()
        .unwrap();
    session
        .until(|event| {
            matches!(
                event,
                Event::Rebuild {
                    progress: RebuildProgress::Succeeded,
                    ..
                }
            )
        })
        .await;
    let second = session.until_running("api").await;
    commands.send(Command::Stop(id("api"))).unwrap();
    let stopped = session
        .until_state("api", |state| *state == ServiceState::Stopped)
        .await;

    assert_ne!(first, second);
    assert_eq!(builds(&session), 2);
    assert_eq!(stopped, ServiceState::Stopped);
    session.shutdown().await;
}

#[tokio::test]
async fn sending_through_a_cloned_sender_after_shutdown_reports_the_supervisor_closed() {
    let mut session = Session::start(|dir| vec![spec("web", dir, "sleep 1000")]);
    session.until_running("web").await;
    let commands = session.handle.commands();

    session.shutdown().await;
    let outcome = timeout(EVENT_TIMEOUT, async {
        loop {
            match commands.send(Command::Rebuild(id("web"))) {
                Err(error) => return error,
                Ok(()) => sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("sender kept accepting commands after shutdown");

    assert_eq!(outcome, SupervisorError::Closed);
    assert_eq!(
        session.handle.send(Command::Stop(id("web"))),
        Err(SupervisorError::Closed)
    );
}
