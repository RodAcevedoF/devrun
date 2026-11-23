#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{
    EVENT_TIMEOUT, Session, id, options, process_exists, read_pids, spec, wait_until_gone,
};
use devrun_supervisor::{Command, LaunchSpec, ServiceState, SupervisorOptions};

const TWO_CHILDREN: &str = "trap '' HUP; sleep 1000 & echo $! >> \"$PIDS\"; sleep 1000 & echo $! >> \"$PIDS\"; echo children-started; wait";

fn with_pid_file(dir: &Path, raw_id: &str, command: &str) -> LaunchSpec {
    let pid_file = dir.join(format!("{raw_id}.pids"));
    LaunchSpec {
        env: BTreeMap::from([("PIDS".to_owned(), pid_file.display().to_string())]),
        ..spec(raw_id, dir, command)
    }
}

async fn group_pids(session: &mut Session, raw_id: &str) -> Vec<u32> {
    session.until_output(raw_id, "children-started").await;
    let leader = session.current_pid(raw_id);
    let mut pids = read_pids(&session.path(&format!("{raw_id}.pids")));
    assert_eq!(pids.len(), 2, "background children were not recorded");
    pids.push(leader);
    pids
}

#[tokio::test]
async fn stop_kills_the_whole_process_group() {
    let mut session = Session::start(|dir| vec![with_pid_file(dir, "web", TWO_CHILDREN)]);
    let pids = group_pids(&mut session, "web").await;

    session.send(Command::Stop(id("web")));
    session
        .until_state("web", |state| *state == ServiceState::Stopped)
        .await;

    assert!(
        pids.iter().all(|pid| !process_exists(*pid)),
        "survivors in {pids:?}"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn stop_escalates_to_sigkill_when_sigterm_is_ignored() {
    let grace_period = Duration::from_millis(400);
    let stubborn = "trap '' TERM HUP; sleep 1000 & echo $! >> \"$PIDS\"; sleep 1000 & echo $! >> \"$PIDS\"; echo children-started; while :; do sleep 1; done";
    let mut session = Session::start_with(
        SupervisorOptions {
            grace_period,
            ..options()
        },
        |dir| vec![with_pid_file(dir, "web", stubborn)],
    );
    let pids = group_pids(&mut session, "web").await;

    let stop_requested = Instant::now();
    session.send(Command::Stop(id("web")));
    session
        .until_state("web", |state| *state == ServiceState::Stopped)
        .await;

    assert!(stop_requested.elapsed() >= grace_period);
    assert!(
        pids.iter().all(|pid| !process_exists(*pid)),
        "survivors in {pids:?}"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn stop_of_an_exited_service_is_a_no_op() {
    let mut session = Session::start(|dir| vec![spec("web", dir, "exit 0")]);
    session
        .until_state("web", |state| matches!(state, ServiceState::Exited { .. }))
        .await;
    let before = session.states_of("web").len();

    session.send(Command::Stop(id("web")));
    session.fence().await;

    assert_eq!(session.states_of("web").len(), before);
    session.shutdown().await;
}

#[tokio::test]
async fn shutdown_stops_everything_in_reverse_dependency_order() {
    let mut session = Session::start(|dir| {
        vec![
            with_pid_file(dir, "db", TWO_CHILDREN),
            LaunchSpec {
                needs: vec![id("db")],
                ..with_pid_file(dir, "api", TWO_CHILDREN)
            },
        ]
    });
    let mut pids = group_pids(&mut session, "db").await;
    pids.extend(group_pids(&mut session, "api").await);

    session.shutdown().await;

    let api_stopping = session.position("api", |state| *state == ServiceState::Stopping);
    let db_stopping = session.position("db", |state| *state == ServiceState::Stopping);
    let api_stopped = session.position("api", |state| *state == ServiceState::Stopped);
    assert!(api_stopping < db_stopping);
    assert!(api_stopped < db_stopping);
    assert!(
        pids.iter().all(|pid| !process_exists(*pid)),
        "survivors in {pids:?}"
    );
    let closed = tokio::time::timeout(EVENT_TIMEOUT, session.handle.recv())
        .await
        .unwrap();
    assert!(closed.is_none());
}

#[tokio::test]
async fn dropping_the_handle_stops_all_process_groups() {
    let mut session = Session::start(|dir| vec![with_pid_file(dir, "web", TWO_CHILDREN)]);
    let pids = group_pids(&mut session, "web").await;

    drop(session.handle);

    wait_until_gone(&pids).await;
}

#[test]
fn dropping_the_runtime_kills_all_process_groups() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let (pids, session) = runtime.block_on(async {
        let mut session = Session::start(|dir| vec![with_pid_file(dir, "web", TWO_CHILDREN)]);
        (group_pids(&mut session, "web").await, session)
    });

    drop(runtime);

    let deadline = Instant::now() + EVENT_TIMEOUT;
    while pids.iter().any(|pid| process_exists(*pid)) {
        assert!(Instant::now() < deadline, "survivors in {pids:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(session);
}
