mod model;
mod terminal;
mod view;

use std::thread;
use std::time::Duration;

use anyhow::bail;
use crossterm::event::{self as terminal_event, Event as TerminalEvent, KeyEventKind};
use devrun_supervisor::{
    Command, Event, LaunchSpec, Supervisor, SupervisorHandle, SupervisorOptions,
};
use ratatui::layout::Rect;
use tokio::sync::mpsc;
use tokio::time::{Instant, sleep_until};

use model::Dashboard;
use terminal::TerminalSession;

const FRAME_INTERVAL: Duration = Duration::from_millis(33);
const INPUT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const INPUT_QUEUE: usize = 64;
const EVENT_BATCH: usize = 512;

pub async fn run(
    repo: String,
    specs: Vec<LaunchSpec>,
    mut interrupts: mpsc::UnboundedReceiver<()>,
) -> anyhow::Result<()> {
    let mut session = TerminalSession::enter()?;
    let size = view::pane_size(session.area()?);
    let options = SupervisorOptions {
        rows: size.rows,
        cols: size.cols,
        ..SupervisorOptions::default()
    };
    let mut handle = Supervisor::spawn(specs.clone(), options)?;
    let mut dashboard = Dashboard::new(repo, &specs, size);
    let outcome = drive(&mut session, &mut handle, &mut dashboard, &mut interrupts).await;
    drop(session);
    if outcome.is_err() {
        stop_everything(&mut handle).await;
    }
    outcome
}

async fn stop_everything(handle: &mut SupervisorHandle) {
    if handle.send(Command::Shutdown).is_err() {
        return;
    }
    while let Some(event) = handle.recv().await {
        if event == Event::ShutdownComplete {
            return;
        }
    }
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Done,
}

struct FrameClock {
    dirty: bool,
    next_frame: Instant,
}

impl FrameClock {
    fn new() -> Self {
        Self {
            dirty: true,
            next_frame: Instant::now(),
        }
    }

    fn touch(&mut self) {
        self.dirty = true;
    }

    fn drawn(&mut self) {
        self.dirty = false;
        self.next_frame = Instant::now() + FRAME_INTERVAL;
    }
}

async fn drive(
    session: &mut TerminalSession,
    handle: &mut SupervisorHandle,
    dashboard: &mut Dashboard,
    interrupts: &mut mpsc::UnboundedReceiver<()>,
) -> anyhow::Result<()> {
    let mut inputs = forward_terminal_input();
    let mut clock = FrameClock::new();
    loop {
        tokio::select! {
            event = handle.recv() => {
                let Some(event) = event else {
                    bail!("the supervisor stopped unexpectedly");
                };
                if absorb(dashboard, handle, event) == Flow::Done {
                    return Ok(());
                }
                clock.touch();
            }
            input = inputs.recv() => {
                let Some(input) = input else {
                    bail!("lost the terminal input");
                };
                if let Some(command) = react(dashboard, handle, &input) {
                    handle.send(command)?;
                }
                clock.touch();
            }
            Some(()) = interrupts.recv() => {
                if let Some(command) = dashboard.quit() {
                    handle.send(command)?;
                }
                clock.touch();
            }
            () = sleep_until(clock.next_frame), if clock.dirty => {
                let statuses = handle.snapshot();
                session.draw(|frame| view::draw(frame, dashboard, &statuses))?;
                clock.drawn();
            }
        }
    }
}

fn absorb(dashboard: &mut Dashboard, handle: &mut SupervisorHandle, first: Event) -> Flow {
    let mut next = Some(first);
    let mut absorbed = 0;
    while let Some(event) = next {
        if event == Event::ShutdownComplete {
            return Flow::Done;
        }
        dashboard.apply(&event);
        absorbed += 1;
        next = if absorbed < EVENT_BATCH {
            handle.try_recv()
        } else {
            None
        };
    }
    Flow::Continue
}

fn react(
    dashboard: &mut Dashboard,
    handle: &SupervisorHandle,
    input: &TerminalEvent,
) -> Option<Command> {
    match *input {
        TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => {
            dashboard.on_key(key, &handle.snapshot())
        }
        TerminalEvent::Resize(cols, rows) => {
            Some(dashboard.resize(view::pane_size(Rect::new(0, 0, cols, rows))))
        }
        _ => None,
    }
}

fn forward_terminal_input() -> mpsc::Receiver<TerminalEvent> {
    let (sender, receiver) = mpsc::channel(INPUT_QUEUE);
    thread::spawn(move || {
        while !sender.is_closed() {
            match terminal_event::poll(INPUT_POLL_INTERVAL) {
                Ok(false) => {}
                Ok(true) => {
                    let delivered = terminal_event::read()
                        .is_ok_and(|input| sender.blocking_send(input).is_ok());
                    if !delivered {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    receiver
}
