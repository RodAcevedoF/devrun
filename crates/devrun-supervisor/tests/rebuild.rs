#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{Session, id, read_pids, spec, wait_until_gone};
use devrun_supervisor::{Command, Event, LaunchSpec, RebuildProgress, ServiceState};

const COUNTED_BUILD: &str = "echo build >> builds.log; ps -o nice= -p $$ | tr -d ' ' > nice.txt";
const SLOW_WHEN_FLAGGED: &str = "echo build >> builds.log; if [ -f slow ]; then sleep 0.5; fi";
const HELD_WHEN_FLAGGED: &str = "echo build >> builds.log; if [ -f hold ]; then while [ ! -f release ]; do sleep 0.02; done; fi";
const CRASHES_ON_REQUEST: &str = "while [ ! -f crash ]; do sleep 0.02; done; rm crash; exit 3";
const HANGS_ONCE_WHEN_FLAGGED: &str =
    "if [ -f hang ]; then rm hang; echo $$ > rebuild.pid; sleep 1000; fi";
const RECORDS_OVERLAP_AND_WINDS_DOWN_SLOWLY_ONCE_WHEN_FLAGGED: &str = "if [ -f rebuild.pid ] && kill -0 $(cat rebuild.pid) 2>/dev/null; then echo overlap >> overlaps.log; fi; if [ -f trap ]; then rm trap; trap 'sleep 0.5; exit 1' TERM; echo $$ > rebuild.pid; while :; do sleep 0.02; done; fi; echo build >> builds.log";
const NICENESS_WITHOUT_SERVICE_PATH: &str =
    "echo build >> builds.log; /bin/ps -o nice= -p $$ > nice.txt";
const FAILS_WHEN_BROKEN: &str = "echo build >> builds.log; if [ -f broken ]; then exit 4; fi";
const CRASHES_LEAVING_A_STUBBORN_CHILD: &str = "(trap '' TERM HUP; while kill -0 $$ 2>/dev/null; do sleep 0.02; done; touch leader-gone; while :; do sleep 0.02; done) & while [ ! -f crash ]; do sleep 0.02; done; rm crash; exit 3";

fn prepared(raw_id: &str, dir: &std::path::Path, prepare: &[&str]) -> LaunchSpec {
    LaunchSpec {
        prepare: prepare.iter().map(|step| (*step).to_owned()).collect(),
        ..spec(raw_id, dir, "sleep 1000")
    }
}

fn line_count(session: &Session, name: &str) -> usize {
    std::fs::read_to_string(session.path(name))
        .unwrap_or_default()
        .lines()
        .count()
}

fn rebuild_progress(session: &Session, raw_id: &str) -> Vec<RebuildProgress> {
    let wanted = id(raw_id);
    session
        .history
        .iter()
        .filter_map(|event| match event {
            Event::Rebuild { service, progress } if *service == wanted => Some(progress.clone()),
            _ => None,
        })
        .collect()
}

fn rebuild_position(session: &Session, raw_id: &str, wanted: &RebuildProgress) -> usize {
    let service_id = id(raw_id);
    session
        .history
        .iter()
        .position(|event| {
            matches!(event, Event::Rebuild { service, progress } if *service == service_id && progress == wanted)
        })
        .unwrap_or_else(|| panic!("rebuild progress not found; history: {:#?}", session.history))
}

async fn until_rebuild(session: &mut Session, raw_id: &str, wanted: fn(&RebuildProgress) -> bool) {
    let service_id = id(raw_id);
    session
        .until(|event| {
            matches!(event, Event::Rebuild { service, progress } if *service == service_id && wanted(progress))
        })
        .await;
}

fn is_running(progress: &RebuildProgress) -> bool {
    matches!(progress, RebuildProgress::Running { .. })
}

fn is_succeeded(progress: &RebuildProgress) -> bool {
    *progress == RebuildProgress::Succeeded
}

fn is_failed(progress: &RebuildProgress) -> bool {
    matches!(progress, RebuildProgress::Failed { .. })
}

#[tokio::test]
async fn rebuild_reruns_prepare_with_lower_priority_while_the_service_keeps_running_then_restarts_it()
 {
    let mut session = Session::start(|dir| vec![prepared("api", dir, &[COUNTED_BUILD])]);
    let first = session.until_running("api").await;
    assert_eq!(
        std::fs::read_to_string(session.path("nice.txt"))
            .unwrap()
            .trim(),
        "0"
    );

    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_succeeded).await;
    let second = session.until_running("api").await;

    assert_ne!(first, second);
    assert_eq!(line_count(&session, "builds.log"), 2);
    assert_eq!(
        std::fs::read_to_string(session.path("nice.txt"))
            .unwrap()
            .trim(),
        "10"
    );
    assert_eq!(
        rebuild_progress(&session, "api"),
        [
            RebuildProgress::Running { step: 1, total: 1 },
            RebuildProgress::Succeeded
        ]
    );
    let succeeded = rebuild_position(&session, "api", &RebuildProgress::Succeeded);
    let stopping = session.position("api", |state| *state == ServiceState::Stopping);
    assert!(
        succeeded < stopping,
        "the service stopped before the rebuild ended"
    );
    session.shutdown().await;
}

#[tokio::test]
async fn failed_rebuild_reports_the_failure_and_keeps_the_service_running() {
    let mut session = Session::start(|dir| {
        vec![prepared(
            "api",
            dir,
            &["echo ok", "if [ -f broken ]; then exit 4; fi"],
        )]
    });
    let pid = session.until_running("api").await;
    std::fs::write(session.path("broken"), "").unwrap();

    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_failed).await;
    session.fence().await;

    assert_eq!(
        rebuild_progress(&session, "api"),
        [
            RebuildProgress::Running { step: 1, total: 2 },
            RebuildProgress::Running { step: 2, total: 2 },
            RebuildProgress::Failed {
                reason: "prepare step 2/2 failed with code 4".to_owned()
            }
        ]
    );
    assert_eq!(session.current_pid("api"), pid);
    assert_eq!(
        session.states_of("api").last(),
        Some(&ServiceState::Running { pid })
    );
    session.shutdown().await;
}

#[tokio::test]
async fn rebuilds_requested_during_a_rebuild_collapse_into_one_rerun_and_one_restart() {
    let mut session = Session::start(|dir| vec![prepared("api", dir, &[SLOW_WHEN_FLAGGED])]);
    let first = session.until_running("api").await;
    std::fs::write(session.path("slow"), "").unwrap();

    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_running).await;
    for _ in 0..3 {
        session.send(Command::Rebuild(id("api")));
    }
    until_rebuild(&mut session, "api", is_succeeded).await;
    let second = session.until_running("api").await;
    session.fence().await;

    assert_ne!(first, second);
    assert_eq!(line_count(&session, "builds.log"), 3);
    assert_eq!(
        rebuild_progress(&session, "api"),
        [
            RebuildProgress::Running { step: 1, total: 1 },
            RebuildProgress::Running { step: 1, total: 1 },
            RebuildProgress::Succeeded
        ]
    );
    let restarts = session
        .states_of("api")
        .iter()
        .filter(|state| **state == ServiceState::Stopping)
        .count();
    assert_eq!(restarts, 1);
    session.shutdown().await;
}

#[tokio::test]
async fn rebuild_requested_while_first_preparing_reruns_prepare_once_before_starting() {
    let mut session = Session::start(|dir| {
        std::fs::write(dir.join("slow"), "").unwrap();
        vec![prepared("api", dir, &[SLOW_WHEN_FLAGGED])]
    });
    session
        .until_state("api", |state| {
            matches!(state, ServiceState::Preparing { .. })
        })
        .await;

    session.send(Command::Rebuild(id("api")));
    session.until_running("api").await;
    session.fence().await;

    assert_eq!(line_count(&session, "builds.log"), 2);
    assert!(rebuild_progress(&session, "api").is_empty());
    let starts = session
        .states_of("api")
        .iter()
        .filter(|state| **state == ServiceState::Starting)
        .count();
    assert_eq!(starts, 1);
    session.shutdown().await;
}

#[tokio::test]
async fn rebuild_of_a_failed_service_prepares_and_starts_it_again() {
    let mut session = Session::start(|dir| {
        vec![prepared(
            "api",
            dir,
            &["echo build >> builds.log; [ -f fixed ] || exit 2"],
        )]
    });
    session
        .until_state("api", |state| matches!(state, ServiceState::Failed { .. }))
        .await;
    std::fs::write(session.path("fixed"), "").unwrap();

    session.send(Command::Rebuild(id("api")));
    session.until_running("api").await;

    assert_eq!(line_count(&session, "builds.log"), 2);
    session.shutdown().await;
}

#[tokio::test]
async fn rebuild_of_a_stopped_service_leaves_it_stopped_and_prepares_on_next_start() {
    let mut session = Session::start(|dir| vec![prepared("api", dir, &[COUNTED_BUILD])]);
    session.until_running("api").await;
    session.send(Command::Stop(id("api")));
    session
        .until_state("api", |state| *state == ServiceState::Stopped)
        .await;

    session.send(Command::Rebuild(id("api")));
    session.fence().await;
    assert_eq!(session.handle.snapshot()[0].state, ServiceState::Stopped);
    assert_eq!(line_count(&session, "builds.log"), 1);

    session.send(Command::Start(id("api")));
    session.until_running("api").await;
    assert_eq!(line_count(&session, "builds.log"), 2);
    assert!(rebuild_progress(&session, "api").is_empty());
    session.shutdown().await;
}

#[tokio::test]
async fn rebuild_without_prepare_steps_restarts_the_service() {
    let mut session = Session::start(|dir| vec![spec("api", dir, "sleep 1000")]);
    let first = session.until_running("api").await;

    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_succeeded).await;
    let second = session.until_running("api").await;

    assert_ne!(first, second);
    assert_eq!(
        rebuild_progress(&session, "api"),
        [RebuildProgress::Succeeded]
    );
    session.shutdown().await;
}

#[tokio::test]
async fn stopping_a_service_cancels_its_rebuild_without_leaking_processes() {
    let mut session = Session::start(|dir| {
        vec![prepared(
            "api",
            dir,
            &["if [ -f hang ]; then echo $$ > rebuild.pid; sleep 1000; fi"],
        )]
    });
    session.until_running("api").await;
    std::fs::write(session.path("hang"), "").unwrap();
    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_running).await;
    let rebuild = wait_for_pid(&session, "rebuild.pid").await;

    session.send(Command::Stop(id("api")));
    session
        .until_state("api", |state| *state == ServiceState::Stopped)
        .await;
    wait_until_gone(&rebuild).await;
    session.fence().await;

    assert_eq!(
        rebuild_progress(&session, "api"),
        [RebuildProgress::Running { step: 1, total: 1 }]
    );
    assert_eq!(session.handle.snapshot()[0].state, ServiceState::Stopped);
    session.shutdown().await;
}

#[tokio::test]
async fn shutdown_during_a_rebuild_terminates_the_rebuild() {
    let mut session = Session::start(|dir| {
        vec![prepared(
            "api",
            dir,
            &["if [ -f hang ]; then echo $$ > rebuild.pid; sleep 1000; fi"],
        )]
    });
    session.until_running("api").await;
    std::fs::write(session.path("hang"), "").unwrap();
    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_running).await;
    let rebuild = wait_for_pid(&session, "rebuild.pid").await;

    session.shutdown().await;

    wait_until_gone(&rebuild).await;
}

#[tokio::test]
async fn rebuild_while_the_service_is_ending_re_prepares_it_on_its_next_start_without_restarting_it()
 {
    let options = devrun_supervisor::SupervisorOptions {
        grace_period: std::time::Duration::from_secs(1),
        ..common::options()
    };
    let mut session = Session::start_with(options, |dir| {
        vec![LaunchSpec {
            prepare: vec![COUNTED_BUILD.to_owned()],
            ..spec("api", dir, CRASHES_LEAVING_A_STUBBORN_CHILD)
        }]
    });
    session.until_running("api").await;
    std::fs::write(session.path("crash"), "").unwrap();
    wait_for_file(&session, "leader-gone").await;
    session.fence().await;

    session.send(Command::Rebuild(id("api")));
    session
        .until_state("api", |state| {
            *state == ServiceState::Exited { code: Some(3) }
        })
        .await;
    session.fence().await;

    assert!(rebuild_progress(&session, "api").is_empty());
    assert_eq!(
        session.handle.snapshot()[0].state,
        ServiceState::Exited { code: Some(3) }
    );
    assert_eq!(line_count(&session, "builds.log"), 1);

    session.send(Command::Start(id("api")));
    session.until_running("api").await;
    let exited = session.position("api", |state| matches!(state, ServiceState::Exited { .. }));
    let preparing = session.position_after("api", exited, |state| {
        matches!(state, ServiceState::Preparing { .. })
    });
    assert!(preparing.is_some(), "the next start did not re-prepare");
    assert_eq!(line_count(&session, "builds.log"), 2);
    session.shutdown().await;
}

#[tokio::test]
async fn a_service_that_crashes_mid_rebuild_starts_again_without_a_second_prepare_when_the_rebuild_succeeds()
 {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec![HELD_WHEN_FLAGGED.to_owned()],
            ..spec("api", dir, CRASHES_ON_REQUEST)
        }]
    });
    let first = session.until_running("api").await;
    std::fs::write(session.path("hold"), "").unwrap();
    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_running).await;

    std::fs::write(session.path("crash"), "").unwrap();
    session
        .until_state("api", |state| {
            *state == ServiceState::Exited { code: Some(3) }
        })
        .await;
    std::fs::write(session.path("release"), "").unwrap();
    until_rebuild(&mut session, "api", is_succeeded).await;
    let second = session.until_running("api").await;
    session.fence().await;

    assert_ne!(first, second);
    assert_eq!(line_count(&session, "builds.log"), 2);
    assert_eq!(
        rebuild_progress(&session, "api"),
        [
            RebuildProgress::Running { step: 1, total: 1 },
            RebuildProgress::Succeeded
        ]
    );
    let exited = session.position("api", |state| matches!(state, ServiceState::Exited { .. }));
    assert_eq!(
        session.position_after("api", exited, |state| {
            matches!(state, ServiceState::Preparing { .. })
        }),
        None
    );
    assert_eq!(
        session.handle.snapshot()[0].state,
        ServiceState::Running { pid: second }
    );
    session.shutdown().await;
}

async fn user_command_on_a_service_that_crashed_mid_rebuild_cancels_the_rebuild(
    command: fn(devrun_supervisor::ServiceId) -> Command,
) {
    let mut session = Session::start(|dir| {
        vec![LaunchSpec {
            prepare: vec![HANGS_ONCE_WHEN_FLAGGED.to_owned()],
            ..spec("api", dir, CRASHES_ON_REQUEST)
        }]
    });
    let first = session.until_running("api").await;
    std::fs::write(session.path("hang"), "").unwrap();
    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_running).await;
    let rebuild = wait_for_pid(&session, "rebuild.pid").await;
    std::fs::write(session.path("crash"), "").unwrap();
    session
        .until_state("api", |state| {
            *state == ServiceState::Exited { code: Some(3) }
        })
        .await;

    session.send(command(id("api")));
    let second = session.until_running("api").await;
    wait_until_gone(&rebuild).await;
    session.fence().await;

    assert_ne!(first, second);
    assert_eq!(
        rebuild_progress(&session, "api"),
        [RebuildProgress::Running { step: 1, total: 1 }]
    );
    let starts = session
        .states_of("api")
        .iter()
        .filter(|state| matches!(state, ServiceState::Running { .. }))
        .count();
    assert_eq!(starts, 2);
    assert!(!session.states_of("api").contains(&ServiceState::Stopping));
    assert_eq!(
        session.handle.snapshot()[0].state,
        ServiceState::Running { pid: second }
    );
    session.shutdown().await;
}

#[tokio::test]
async fn restarting_a_service_that_crashed_mid_rebuild_cancels_the_rebuild_and_starts_it_once() {
    user_command_on_a_service_that_crashed_mid_rebuild_cancels_the_rebuild(Command::Restart).await;
}

#[tokio::test]
async fn starting_a_service_that_crashed_mid_rebuild_cancels_the_rebuild_and_starts_it_once() {
    user_command_on_a_service_that_crashed_mid_rebuild_cancels_the_rebuild(Command::Start).await;
}

#[tokio::test]
async fn starting_a_serving_service_keeps_its_rebuild_running_and_restarts_it_once_rebuilt() {
    let mut session = Session::start(|dir| vec![prepared("api", dir, &[HELD_WHEN_FLAGGED])]);
    let first = session.until_running("api").await;
    std::fs::write(session.path("hold"), "").unwrap();
    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_running).await;

    session.send(Command::Start(id("api")));
    session.fence().await;
    std::fs::write(session.path("release"), "").unwrap();
    until_rebuild(&mut session, "api", is_succeeded).await;
    let second = session.until_running("api").await;
    session.fence().await;

    assert_ne!(first, second);
    assert_eq!(line_count(&session, "builds.log"), 2);
    assert_eq!(
        rebuild_progress(&session, "api"),
        [
            RebuildProgress::Running { step: 1, total: 1 },
            RebuildProgress::Succeeded
        ]
    );
    let restarts = session
        .states_of("api")
        .iter()
        .filter(|state| **state == ServiceState::Stopping)
        .count();
    assert_eq!(restarts, 1);
    assert_eq!(
        session.handle.snapshot()[0].state,
        ServiceState::Running { pid: second }
    );
    session.shutdown().await;
}

async fn cancel_a_rebuild_that_winds_down_slowly(session: &mut Session) -> Vec<u32> {
    session.until_running("api").await;
    std::fs::write(session.path("trap"), "").unwrap();
    session.send(Command::Rebuild(id("api")));
    until_rebuild(session, "api", is_running).await;
    let cancelled = wait_for_pid(session, "rebuild.pid").await;
    session.send(Command::Restart(id("api")));
    session
        .until_state("api", |state| {
            matches!(state, ServiceState::Preparing { .. })
        })
        .await;
    cancelled
}

fn slow_wind_down_session() -> Session {
    let options = devrun_supervisor::SupervisorOptions {
        grace_period: std::time::Duration::from_secs(5),
        ..common::options()
    };
    Session::start_with(options, |dir| {
        vec![prepared(
            "api",
            dir,
            &[RECORDS_OVERLAP_AND_WINDS_DOWN_SLOWLY_ONCE_WHEN_FLAGGED],
        )]
    })
}

#[tokio::test]
async fn no_prepare_starts_until_the_cancelled_rebuild_has_wound_down_and_a_rebuild_requested_meanwhile_is_kept()
 {
    let mut session = slow_wind_down_session();
    let cancelled = cancel_a_rebuild_that_winds_down_slowly(&mut session).await;
    assert!(common::process_exists(cancelled[0]));

    session.send(Command::Rebuild(id("api")));
    let restarted = session.until_running("api").await;
    assert!(!common::process_exists(cancelled[0]));
    session.fence().await;

    assert!(!session.path("overlaps.log").exists());
    assert_eq!(line_count(&session, "builds.log"), 2);
    assert_eq!(
        rebuild_progress(&session, "api"),
        [RebuildProgress::Running { step: 1, total: 1 }]
    );
    assert_eq!(
        session.handle.snapshot()[0].state,
        ServiceState::Running { pid: restarted }
    );
    session.shutdown().await;
}

#[tokio::test]
async fn stopping_a_service_whose_prepare_waits_for_a_wind_down_never_runs_that_prepare() {
    let mut session = slow_wind_down_session();
    let cancelled = cancel_a_rebuild_that_winds_down_slowly(&mut session).await;

    session.send(Command::Stop(id("api")));
    session
        .until_state("api", |state| *state == ServiceState::Stopped)
        .await;
    wait_until_gone(&cancelled).await;
    session.fence().await;

    assert_eq!(session.handle.snapshot()[0].state, ServiceState::Stopped);
    assert_eq!(line_count(&session, "builds.log"), 1);
    assert!(!session.path("overlaps.log").exists());
    session.shutdown().await;
}

#[tokio::test]
async fn restarting_a_service_whose_prepare_waits_for_a_wind_down_still_prepares_and_starts_it() {
    let mut session = slow_wind_down_session();
    let cancelled = cancel_a_rebuild_that_winds_down_slowly(&mut session).await;

    session.send(Command::Restart(id("api")));
    session.until_running("api").await;
    session.fence().await;

    assert!(!session.path("overlaps.log").exists());
    assert!(cancelled.iter().all(|pid| !common::process_exists(*pid)));
    assert!(matches!(
        session.handle.snapshot()[0].state,
        ServiceState::Running { .. }
    ));
    session.shutdown().await;
}

#[tokio::test]
async fn a_failed_rebuild_makes_the_next_restart_prepare_again_instead_of_starting_the_broken_build()
 {
    let mut session = Session::start(|dir| vec![prepared("api", dir, &[FAILS_WHEN_BROKEN])]);
    session.until_running("api").await;
    std::fs::write(session.path("broken"), "").unwrap();
    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", is_failed).await;

    session.send(Command::Restart(id("api")));
    let failed = session
        .until_state("api", |state| matches!(state, ServiceState::Failed { .. }))
        .await;
    session.fence().await;

    assert_eq!(
        failed,
        ServiceState::Failed {
            reason: "prepare step 1/1 failed with code 4".to_owned()
        }
    );
    assert_eq!(line_count(&session, "builds.log"), 3);
    let stopped = session.position("api", |state| *state == ServiceState::Stopped);
    assert_eq!(
        session.position_after("api", stopped, |state| {
            matches!(state, ServiceState::Running { .. })
        }),
        None
    );
    session.shutdown().await;
}

#[tokio::test]
async fn rebuild_runs_niced_even_when_the_service_path_has_no_nice() {
    let mut session = Session::start(|dir| {
        let mut service = LaunchSpec {
            prepare: vec![NICENESS_WITHOUT_SERVICE_PATH.to_owned()],
            ..spec("api", dir, "exec /bin/sleep 1000")
        };
        service
            .env
            .insert("PATH".to_owned(), dir.display().to_string());
        vec![service]
    });
    let first = session.until_running("api").await;

    session.send(Command::Rebuild(id("api")));
    until_rebuild(&mut session, "api", |progress| !is_running(progress)).await;
    assert_eq!(
        rebuild_progress(&session, "api"),
        [
            RebuildProgress::Running { step: 1, total: 1 },
            RebuildProgress::Succeeded
        ]
    );
    let second = session.until_running("api").await;

    assert_ne!(first, second);
    assert_eq!(line_count(&session, "builds.log"), 2);
    assert_eq!(
        std::fs::read_to_string(session.path("nice.txt"))
            .unwrap()
            .trim(),
        "10"
    );
    session.shutdown().await;
}

async fn wait_for_file(session: &Session, name: &str) {
    let path = session.path(name);
    tokio::time::timeout(common::EVENT_TIMEOUT, async {
        while !path.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{name} was never written"));
}

async fn wait_for_pid(session: &Session, name: &str) -> Vec<u32> {
    let path = session.path(name);
    tokio::time::timeout(common::EVENT_TIMEOUT, async {
        loop {
            let pids = read_pids(&path);
            if !pids.is_empty() {
                return pids;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the rebuild never wrote its pid")
}
