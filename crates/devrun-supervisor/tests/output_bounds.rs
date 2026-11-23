#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use common::{EVENT_TIMEOUT, Session, id, options, spec};
use devrun_supervisor::{Command, Event, ServiceId, SupervisorOptions};
use tokio::time::{sleep, timeout};

const LIMIT: usize = 64 * 1024;
const FLOOD_BYTES: usize = 2_000_000;

async fn wait_for_file(path: &Path) {
    timeout(EVENT_TIMEOUT, async {
        while !path.exists() {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("marker file never appeared");
}

#[derive(Default)]
struct Drained {
    chatty_bytes: usize,
    chatty_dropped: u64,
    quiet_text: String,
}

fn drain(session: &mut Session) -> Drained {
    let mut drained = Drained::default();
    while let Some(event) = session.handle.try_recv() {
        match event {
            Event::Output { service, bytes, .. } if service == id("chatty") => {
                drained.chatty_bytes += bytes.len();
            }
            Event::Output { bytes, .. } => {
                drained
                    .quiet_text
                    .push_str(&String::from_utf8_lossy(&bytes));
            }
            Event::OutputDropped { service, bytes } => {
                assert_eq!(service, id("chatty"));
                drained.chatty_dropped += bytes;
            }
            _ => {}
        }
    }
    drained
}

#[tokio::test]
async fn stalled_consumer_keeps_only_the_newest_output_per_service() {
    let flood = format!(
        "head -c {FLOOD_BYTES} /dev/zero | tr '\\0' x; touch flooded; read line; echo \"got:$line\"; sleep 1000"
    );
    let mut session = Session::start_with(
        SupervisorOptions {
            output_buffer_bytes: LIMIT,
            ..options()
        },
        |dir| {
            vec![
                spec("quiet", dir, "echo quiet-line; sleep 1000"),
                spec("chatty", dir, &flood),
            ]
        },
    );
    wait_for_file(&session.path("flooded")).await;
    sleep(Duration::from_millis(300)).await;

    let drained = drain(&mut session);

    assert!(drained.chatty_bytes <= LIMIT, "{}", drained.chatty_bytes);
    assert!(drained.chatty_dropped > 0);
    assert_eq!(
        drained.chatty_bytes + usize::try_from(drained.chatty_dropped).unwrap(),
        FLOOD_BYTES
    );
    assert!(drained.quiet_text.contains("quiet-line"));

    session.send(Command::Input {
        service: id("chatty"),
        bytes: b"again\n".to_vec(),
    });
    session.until_output("chatty", "got:again").await;
    session.shutdown().await;
}

#[tokio::test]
async fn a_keeping_up_consumer_receives_every_byte_without_drop_notices() {
    let mut session = Session::start(|dir| {
        vec![spec(
            "chatty",
            dir,
            "head -c 300000 /dev/zero | tr '\\0' x; echo; echo flood-done",
        )]
    });

    session.until_output("chatty", "flood-done").await;

    let output = session.output_of("chatty", devrun_supervisor::OutputSource::Service);
    assert_eq!(output.matches('x').count(), 300_000);
    assert!(
        !session
            .history
            .iter()
            .any(|event| matches!(event, Event::OutputDropped { .. }))
    );
    session.shutdown().await;
}

#[tokio::test]
async fn stalled_consumer_holds_only_the_latest_usage_per_service() {
    let mut session = Session::start_with(
        SupervisorOptions {
            usage_interval: Some(Duration::from_millis(250)),
            ..options()
        },
        |dir| {
            vec![
                spec("api", dir, "sleep 1000"),
                spec("web", dir, "sleep 1000"),
            ]
        },
    );
    sleep(Duration::from_millis(1500)).await;

    let mut usages: BTreeMap<ServiceId, usize> = BTreeMap::new();
    while let Some(event) = session.handle.try_recv() {
        if let Event::Usage { service, .. } = event {
            *usages.entry(service).or_default() += 1;
        }
    }

    assert_eq!(usages, BTreeMap::from([(id("api"), 1), (id("web"), 1)]));
    session.shutdown().await;
}
