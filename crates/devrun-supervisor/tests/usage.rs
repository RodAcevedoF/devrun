#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::{Session, id, options, spec};
use devrun_supervisor::{Command, Event, ResourceUsage, ServiceState, SupervisorOptions};
use tokio::time::sleep;

const SAMPLE_EVERY: Duration = Duration::from_millis(250);

fn sampled() -> SupervisorOptions {
    SupervisorOptions {
        usage_interval: Some(SAMPLE_EVERY),
        ..options()
    }
}

async fn next_usage(
    session: &mut Session,
    raw_id: &str,
    accept: impl Fn(&ResourceUsage) -> bool,
) -> ResourceUsage {
    let wanted = id(raw_id);
    match session
        .until(|event| {
            matches!(event, Event::Usage { service, usage } if *service == wanted && accept(usage))
        })
        .await
    {
        Event::Usage { usage, .. } => usage,
        _ => unreachable!(),
    }
}

fn usage_events_after(session: &Session, position: usize) -> usize {
    session.history[position..]
        .iter()
        .filter(|event| matches!(event, Event::Usage { .. }))
        .count()
}

#[tokio::test]
async fn usage_covers_every_process_in_the_service_tree() {
    let mut session = Session::start_with(sampled(), |dir| {
        vec![spec("api", dir, "sleep 1000 & sleep 1000 & wait")]
    });

    let usage = next_usage(&mut session, "api", |usage| usage.processes == 3).await;

    assert!(usage.rss_bytes > 0, "{usage:?}");
    session.shutdown().await;
}

#[tokio::test]
async fn busy_services_report_their_cpu_share() {
    let mut session = Session::start_with(sampled(), |dir| {
        vec![spec("busy", dir, "while :; do :; done")]
    });

    let usage = next_usage(&mut session, "busy", |usage| usage.cpu_permille >= 300).await;

    assert_eq!(usage.processes, 1);
    session.shutdown().await;
}

#[tokio::test]
async fn stopped_services_stop_reporting_usage() {
    let mut session = Session::start_with(sampled(), |dir| vec![spec("api", dir, "sleep 1000")]);
    next_usage(&mut session, "api", |_| true).await;

    session.send(Command::Stop(id("api")));
    session
        .until_state("api", |state| *state == ServiceState::Stopped)
        .await;
    let stopped_at = session.history.len();
    sleep(SAMPLE_EVERY * 3).await;
    session.fence().await;

    assert_eq!(usage_events_after(&session, stopped_at), 0);
    session.shutdown().await;
}

#[tokio::test]
async fn sampling_is_off_without_an_interval() {
    let mut session = Session::start_with(
        SupervisorOptions {
            usage_interval: None,
            ..options()
        },
        |dir| vec![spec("api", dir, "echo up; sleep 1000")],
    );
    session.until_output("api", "up").await;
    sleep(Duration::from_millis(600)).await;
    session.fence().await;

    assert_eq!(usage_events_after(&session, 0), 0);
    session.shutdown().await;
}
