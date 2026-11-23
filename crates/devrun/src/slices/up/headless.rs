use std::collections::HashMap;
use std::io::{self, Write};

use anyhow::bail;
use console::{Color, Style};
use devrun_supervisor::{
    Command, Event, LaunchSpec, ServiceId, ServiceStatus, Supervisor, SupervisorHandle,
    SupervisorOptions,
};
use tokio::sync::mpsc::UnboundedReceiver;

use super::lines::LineAssembler;
use super::status;
use crate::ui;

const ROWS: u16 = 48;
const COLS: u16 = 160;
const PALETTE: [Color; 6] = [
    Color::Cyan,
    Color::Magenta,
    Color::Yellow,
    Color::Green,
    Color::Blue,
    Color::Red,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ending {
    Finished,
    Interrupted,
}

pub async fn run(
    specs: Vec<LaunchSpec>,
    mut interrupts: UnboundedReceiver<()>,
) -> anyhow::Result<()> {
    let options = SupervisorOptions {
        rows: ROWS,
        cols: COLS,
        usage_interval: None,
        ..SupervisorOptions::default()
    };
    let mut printer = Printer::new(specs.iter().map(|spec| spec.id.clone()));
    let mut handle = Supervisor::spawn(specs, options)?;
    let mut ending = None;
    loop {
        tokio::select! {
            event = handle.recv() => match event {
                None => bail!("the supervisor stopped unexpectedly"),
                Some(Event::ShutdownComplete) => break,
                Some(event) => {
                    let output_open = printer.show(&event).is_ok();
                    if ending.is_none() {
                        ending = next_ending(&handle, output_open);
                        request_shutdown(&handle, ending)?;
                    }
                }
            },
            Some(()) = interrupts.recv(), if ending.is_none() => {
                ending = Some(Ending::Interrupted);
                request_shutdown(&handle, ending)?;
            }
        }
    }
    printer.finish();
    summarize(ending, &handle.snapshot())
}

fn next_ending(handle: &SupervisorHandle, output_open: bool) -> Option<Ending> {
    if !output_open {
        Some(Ending::Interrupted)
    } else if handle
        .snapshot()
        .iter()
        .all(|status| status::has_ended(&status.state))
    {
        Some(Ending::Finished)
    } else {
        None
    }
}

fn request_shutdown(handle: &SupervisorHandle, ending: Option<Ending>) -> anyhow::Result<()> {
    if ending.is_some() {
        handle.send(Command::Shutdown)?;
    }
    Ok(())
}

fn summarize(ending: Option<Ending>, statuses: &[ServiceStatus]) -> anyhow::Result<()> {
    let count = statuses.len();
    let noun = status::services_noun(count);
    if ending == Some(Ending::Interrupted) {
        report(&format!("stopped {count} {noun}"));
        return Ok(());
    }
    let unclean: Vec<&str> = statuses
        .iter()
        .filter(|status| !status::finished_cleanly(&status.state))
        .map(|status| status.id.as_str())
        .collect();
    if unclean.is_empty() {
        report(&format!("{count} {noun} finished cleanly"));
        Ok(())
    } else {
        bail!("not every service finished cleanly: {}", unclean.join(", "))
    }
}

fn report(summary: &str) {
    let _ = writeln!(io::stdout().lock(), "{summary}");
}

struct Printer {
    width: usize,
    styles: HashMap<ServiceId, Style>,
    lines: HashMap<ServiceId, LineAssembler>,
    closed: bool,
}

impl Printer {
    fn new(ids: impl Iterator<Item = ServiceId>) -> Self {
        let styles: HashMap<ServiceId, Style> = ids
            .zip(PALETTE.iter().cycle())
            .map(|(id, color)| (id, Style::new().fg(*color).bold()))
            .collect();
        Self {
            width: styles
                .keys()
                .map(|id| id.as_str().chars().count())
                .max()
                .unwrap_or(0),
            styles,
            lines: HashMap::new(),
            closed: false,
        }
    }

    fn show(&mut self, event: &Event) -> io::Result<()> {
        match event {
            Event::Output { service, bytes, .. } => {
                let lines = self.lines.entry(service.clone()).or_default().push(bytes);
                lines
                    .into_iter()
                    .try_for_each(|line| self.write_output(service, &line))
            }
            Event::OutputDropped { service, bytes } => {
                self.write_note(service, &format!("dropped {bytes} bytes of output"))
            }
            Event::State { service, state } => {
                if status::has_ended(state) {
                    self.flush_partial(service)?;
                }
                self.write_note(service, &status::describe(state))
            }
            Event::Usage { .. } | Event::Rejected { .. } | Event::ShutdownComplete => Ok(()),
        }
    }

    fn finish(&mut self) {
        let services: Vec<ServiceId> = self.lines.keys().cloned().collect();
        for service in services {
            let _ = self.flush_partial(&service);
        }
    }

    fn flush_partial(&mut self, service: &ServiceId) -> io::Result<()> {
        match self.lines.get_mut(service).and_then(LineAssembler::finish) {
            Some(line) => self.write_output(service, &line),
            None => Ok(()),
        }
    }

    fn write_output(&mut self, service: &ServiceId, line: &str) -> io::Result<()> {
        let name = self.padded(service);
        let styled = self
            .styles
            .get(service)
            .map_or_else(|| name.clone(), |style| style.apply_to(&name).to_string());
        self.write(&format!("{styled} | {line}"))
    }

    fn write_note(&mut self, service: &ServiceId, note: &str) -> io::Result<()> {
        let line = format!("{} · {note}", self.padded(service));
        self.write(&ui::dim_style().apply_to(line).to_string())
    }

    fn padded(&self, service: &ServiceId) -> String {
        format!("{:<width$}", service.as_str(), width = self.width)
    }

    fn write(&mut self, line: &str) -> io::Result<()> {
        if self.closed {
            return Ok(());
        }
        let written = writeln!(io::stdout().lock(), "{line}");
        self.closed = written.is_err();
        written
    }
}
