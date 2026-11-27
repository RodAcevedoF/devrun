use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use devrun_supervisor::{
    Command, Event, LaunchSpec, RebuildProgress, ResourceUsage, ServiceId, ServiceState,
    ServiceStatus,
};
use tui_term::vt100;

use super::super::status;

pub const SCROLLBACK_LINES: usize = 2000;
const ESCAPE: u8 = 0x1b;
const DELETE: u8 = 0x7f;
const CONTROL_MASK: u8 = 0x1f;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneSize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Browse,
    Input,
    Stopping,
}

pub struct Pane {
    pub id: ServiceId,
    pub ports: Vec<u16>,
    terminal: vt100::Parser,
    usage: Option<ResourceUsage>,
    rebuild: Option<RebuildNotice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RebuildNotice {
    Running { step: usize, total: usize },
    Failed { reason: String },
}

impl RebuildNotice {
    fn unfinished(progress: &RebuildProgress) -> Option<Self> {
        match progress {
            RebuildProgress::Running { step, total } => Some(Self::Running {
                step: *step,
                total: *total,
            }),
            RebuildProgress::Succeeded => None,
            RebuildProgress::Failed { reason } => Some(Self::Failed {
                reason: reason.clone(),
            }),
        }
    }
}

impl Pane {
    pub fn usage(&self) -> Option<ResourceUsage> {
        self.usage
    }

    pub fn rebuild(&self) -> Option<&RebuildNotice> {
        self.rebuild.as_ref()
    }

    pub fn screen(&self) -> &vt100::Screen {
        self.terminal.screen()
    }

    pub fn scrolled(&self) -> usize {
        self.terminal.screen().scrollback()
    }
}

pub struct Dashboard {
    repo: String,
    panes: Vec<Pane>,
    selected: usize,
    mode: Mode,
    size: PaneSize,
}

impl Dashboard {
    pub fn new(repo: String, specs: &[LaunchSpec], size: PaneSize) -> Self {
        let panes = specs
            .iter()
            .map(|spec| Pane {
                id: spec.id.clone(),
                ports: spec.ports.clone(),
                terminal: vt100::Parser::new(size.rows, size.cols, SCROLLBACK_LINES),
                usage: None,
                rebuild: None,
            })
            .collect();
        Self {
            repo,
            panes,
            selected: 0,
            mode: Mode::Browse,
            size,
        }
    }

    pub fn repo(&self) -> &str {
        &self.repo
    }

    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_pane(&self) -> &Pane {
        &self.panes[self.selected]
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn apply(&mut self, event: &Event) {
        match event {
            Event::Output { service, bytes, .. } => {
                if let Some(pane) = self.pane_mut(service) {
                    pane.terminal.process(bytes);
                }
            }
            Event::OutputDropped { service, bytes } => {
                if let Some(pane) = self.pane_mut(service) {
                    let notice =
                        format!("\r\n\x1b[2m[devrun] {bytes} bytes of output dropped\x1b[0m\r\n");
                    pane.terminal.process(notice.as_bytes());
                }
            }
            Event::Usage { service, usage } => {
                if let Some(pane) = self.pane_mut(service) {
                    pane.usage = Some(*usage);
                }
            }
            Event::State { service, state } => {
                if !status::is_live(state)
                    && let Some(pane) = self.pane_mut(service)
                {
                    pane.usage = None;
                    pane.rebuild = None;
                }
            }
            Event::Rebuild { service, progress } => {
                if let Some(pane) = self.pane_mut(service) {
                    pane.rebuild = RebuildNotice::unfinished(progress);
                }
            }
            Event::Rejected { .. } | Event::ShutdownComplete => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, statuses: &[ServiceStatus]) -> Option<Command> {
        match self.mode {
            Mode::Browse => self.browse(key, statuses),
            Mode::Input => self.type_into_service(key),
            Mode::Stopping => None,
        }
    }

    pub fn quit(&mut self) -> Option<Command> {
        if self.mode == Mode::Stopping {
            return None;
        }
        self.mode = Mode::Stopping;
        Some(Command::Shutdown)
    }

    pub fn resize(&mut self, size: PaneSize) -> Command {
        self.size = size;
        for pane in &mut self.panes {
            pane.terminal.screen_mut().set_size(size.rows, size.cols);
        }
        Command::Resize {
            rows: size.rows,
            cols: size.cols,
        }
    }

    fn browse(&mut self, key: KeyEvent, statuses: &[ServiceStatus]) -> Option<Command> {
        let selected = self.selected_pane().id.clone();
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => self.quit(),
            KeyCode::Char('q') => self.quit(),
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.select_next(1),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
                self.select_next(self.panes.len() - 1)
            }
            KeyCode::Char('r') => Some(Command::Restart(selected)),
            KeyCode::Char('s') => toggle(selected, statuses),
            KeyCode::Char('i') => {
                self.mode = Mode::Input;
                None
            }
            KeyCode::PageUp => self.scroll(Scroll::Back),
            KeyCode::PageDown => self.scroll(Scroll::Forward),
            _ => None,
        }
    }

    fn type_into_service(&mut self, key: KeyEvent) -> Option<Command> {
        if key.code == KeyCode::Esc {
            self.mode = Mode::Browse;
            return None;
        }
        key_bytes(key).map(|bytes| Command::Input {
            service: self.selected_pane().id.clone(),
            bytes,
        })
    }

    fn select_next(&mut self, step: usize) -> Option<Command> {
        self.selected = (self.selected + step) % self.panes.len();
        None
    }

    fn scroll(&mut self, direction: Scroll) -> Option<Command> {
        let page = usize::from(self.size.rows / 2).max(1);
        let screen = self.panes[self.selected].terminal.screen_mut();
        let offset = match direction {
            Scroll::Back => screen.scrollback().saturating_add(page),
            Scroll::Forward => screen.scrollback().saturating_sub(page),
        };
        screen.set_scrollback(offset);
        None
    }

    fn pane_mut(&mut self, service: &ServiceId) -> Option<&mut Pane> {
        self.panes.iter_mut().find(|pane| pane.id == *service)
    }
}

#[derive(Clone, Copy)]
enum Scroll {
    Back,
    Forward,
}

fn toggle(service: ServiceId, statuses: &[ServiceStatus]) -> Option<Command> {
    let state = &statuses.iter().find(|status| status.id == service)?.state;
    if *state == ServiceState::Stopping {
        None
    } else if status::is_active(state) {
        Some(Command::Stop(service))
    } else {
        Some(Command::Start(service))
    }
}

fn key_bytes(key: KeyEvent) -> Option<Vec<u8>> {
    let sequence: &[u8] = match key.code {
        KeyCode::Char(character) => return Some(character_bytes(character, key.modifiers)),
        KeyCode::Enter => b"\r",
        KeyCode::Backspace => &[DELETE],
        KeyCode::Tab => b"\t",
        KeyCode::BackTab => b"\x1b[Z",
        KeyCode::Up => b"\x1b[A",
        KeyCode::Down => b"\x1b[B",
        KeyCode::Right => b"\x1b[C",
        KeyCode::Left => b"\x1b[D",
        KeyCode::Home => b"\x1b[H",
        KeyCode::End => b"\x1b[F",
        KeyCode::Delete => b"\x1b[3~",
        KeyCode::PageUp => b"\x1b[5~",
        KeyCode::PageDown => b"\x1b[6~",
        _ => return None,
    };
    Some(sequence.to_vec())
}

fn character_bytes(character: char, modifiers: KeyModifiers) -> Vec<u8> {
    if modifiers.contains(KeyModifiers::CONTROL) && character.is_ascii_alphabetic() {
        return vec![(character.to_ascii_lowercase() as u8) & CONTROL_MASK];
    }
    let mut bytes = Vec::with_capacity(5);
    if modifiers.contains(KeyModifiers::ALT) {
        bytes.push(ESCAPE);
    }
    bytes.extend_from_slice(character.encode_utf8(&mut [0; 4]).as_bytes());
    bytes
}

#[cfg(test)]
pub mod tests {
    use devrun_supervisor::{OutputSource, ResourceUsage};

    use super::*;

    pub fn sample_size() -> PaneSize {
        PaneSize { rows: 10, cols: 40 }
    }

    pub fn dashboard_for(ids: &[&str]) -> Dashboard {
        let specs: Vec<LaunchSpec> = ids
            .iter()
            .map(|id| LaunchSpec::new(ServiceId::new(*id), "/repo", "true"))
            .collect();
        Dashboard::new("jaspy-monorepo".to_owned(), &specs, sample_size())
    }

    pub fn statuses(states: &[(&str, ServiceState)]) -> Vec<ServiceStatus> {
        states
            .iter()
            .map(|(id, state)| ServiceStatus {
                id: ServiceId::new(*id),
                state: state.clone(),
            })
            .collect()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn char_key(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }

    fn running(ids: &[&str]) -> Vec<ServiceStatus> {
        statuses(
            &ids.iter()
                .map(|id| (*id, ServiceState::Running { pid: 1 }))
                .collect::<Vec<_>>(),
        )
    }

    fn output(id: &str, text: &str) -> Event {
        Event::Output {
            service: ServiceId::new(id),
            source: OutputSource::Service,
            bytes: text.as_bytes().to_vec(),
        }
    }

    fn screen_text(dashboard: &Dashboard) -> String {
        dashboard.selected_pane().screen().contents()
    }

    #[test]
    fn selection_moves_with_arrows_vim_keys_and_tab_and_wraps_around() {
        let mut dashboard = dashboard_for(&["db", "api", "web"]);
        let states = running(&["db", "api", "web"]);

        dashboard.on_key(key(KeyCode::Up), &states);
        assert_eq!(dashboard.selected(), 2);
        dashboard.on_key(char_key('j'), &states);
        assert_eq!(dashboard.selected(), 0);
        dashboard.on_key(key(KeyCode::Tab), &states);
        assert_eq!(dashboard.selected(), 1);
        dashboard.on_key(key(KeyCode::Down), &states);
        dashboard.on_key(char_key('k'), &states);
        assert_eq!(dashboard.selected(), 1);
        dashboard.on_key(key(KeyCode::BackTab), &states);
        assert_eq!(dashboard.selected(), 0);
    }

    #[test]
    fn r_restarts_the_selected_service() {
        let mut dashboard = dashboard_for(&["db", "api"]);
        let states = running(&["db", "api"]);
        dashboard.on_key(key(KeyCode::Down), &states);

        assert_eq!(
            dashboard.on_key(char_key('r'), &states),
            Some(Command::Restart(ServiceId::new("api")))
        );
    }

    #[test]
    fn s_stops_active_services_and_starts_ended_ones() {
        let mut dashboard = dashboard_for(&["db", "api", "web"]);
        let states = statuses(&[
            ("db", ServiceState::Ready { pid: 1 }),
            ("api", ServiceState::Exited { code: Some(1) }),
            ("web", ServiceState::Stopping),
        ]);

        assert_eq!(
            dashboard.on_key(char_key('s'), &states),
            Some(Command::Stop(ServiceId::new("db")))
        );
        dashboard.on_key(key(KeyCode::Down), &states);
        assert_eq!(
            dashboard.on_key(char_key('s'), &states),
            Some(Command::Start(ServiceId::new("api")))
        );
        dashboard.on_key(key(KeyCode::Down), &states);
        assert_eq!(dashboard.on_key(char_key('s'), &states), None);
    }

    #[test]
    fn input_mode_forwards_keystrokes_to_the_selected_pty_until_escape() {
        let mut dashboard = dashboard_for(&["db", "api"]);
        let states = running(&["db", "api"]);
        dashboard.on_key(key(KeyCode::Down), &states);
        let input = |bytes: &[u8]| {
            Some(Command::Input {
                service: ServiceId::new("api"),
                bytes: bytes.to_vec(),
            })
        };

        assert_eq!(dashboard.on_key(char_key('i'), &states), None);
        assert_eq!(dashboard.mode(), Mode::Input);
        assert_eq!(dashboard.on_key(char_key('q'), &states), input(b"q"));
        assert_eq!(dashboard.on_key(key(KeyCode::Enter), &states), input(b"\r"));
        assert_eq!(
            dashboard.on_key(key(KeyCode::Up), &states),
            input(b"\x1b[A")
        );
        assert_eq!(
            dashboard.on_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                &states
            ),
            input(&[0x03])
        );
        assert_eq!(dashboard.on_key(key(KeyCode::Esc), &states), None);
        assert_eq!(dashboard.mode(), Mode::Browse);
        assert_eq!(dashboard.selected(), 1);
    }

    #[test]
    fn q_and_ctrl_c_quit_once_and_ignore_keys_while_stopping() {
        let states = running(&["db"]);
        for quit in [
            char_key('q'),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let mut dashboard = dashboard_for(&["db"]);

            assert_eq!(dashboard.on_key(quit, &states), Some(Command::Shutdown));
            assert_eq!(dashboard.mode(), Mode::Stopping);
            assert_eq!(dashboard.on_key(char_key('r'), &states), None);
            assert_eq!(dashboard.quit(), None);
        }
    }

    #[test]
    fn output_is_rendered_into_the_pane_of_its_service() {
        let mut dashboard = dashboard_for(&["db", "api"]);

        dashboard.apply(&output("api", "\x1b[32mlistening\x1b[0m on 3000\r\n"));
        dashboard.apply(&output("db", "db-line\r\n"));

        assert!(screen_text(&dashboard).contains("db-line"));
        dashboard.on_key(key(KeyCode::Down), &running(&["db", "api"]));
        assert!(screen_text(&dashboard).contains("listening on 3000"));
        assert!(!screen_text(&dashboard).contains("db-line"));
    }

    #[test]
    fn dropped_output_leaves_a_visible_notice() {
        let mut dashboard = dashboard_for(&["db"]);

        dashboard.apply(&Event::OutputDropped {
            service: ServiceId::new("db"),
            bytes: 4096,
        });

        assert!(screen_text(&dashboard).contains("4096 bytes of output dropped"));
    }

    fn usage(service: &str, cpu_permille: u32) -> Event {
        Event::Usage {
            service: ServiceId::new(service),
            usage: ResourceUsage {
                cpu_permille,
                rss_bytes: 4096,
                processes: 2,
            },
        }
    }

    #[test]
    fn usage_is_kept_per_pane_and_replaced_by_newer_samples() {
        let mut dashboard = dashboard_for(&["db", "api"]);

        dashboard.apply(&usage("api", 10));
        dashboard.apply(&usage("api", 30));

        assert_eq!(dashboard.panes()[0].usage(), None);
        assert_eq!(
            dashboard.panes()[1].usage().map(|usage| usage.cpu_permille),
            Some(30)
        );
    }

    #[test]
    fn usage_is_forgotten_once_the_service_is_no_longer_live() {
        let mut dashboard = dashboard_for(&["api"]);
        dashboard.apply(&usage("api", 10));

        dashboard.apply(&Event::State {
            service: ServiceId::new("api"),
            state: ServiceState::Ready { pid: 1 },
        });
        assert!(dashboard.panes()[0].usage().is_some());
        dashboard.apply(&Event::State {
            service: ServiceId::new("api"),
            state: ServiceState::Stopping,
        });

        assert_eq!(dashboard.panes()[0].usage(), None);
    }

    fn rebuild(id: &str, progress: RebuildProgress) -> Event {
        Event::Rebuild {
            service: ServiceId::new(id),
            progress,
        }
    }

    #[test]
    fn rebuild_progress_is_kept_until_it_succeeds() {
        let mut dashboard = dashboard_for(&["api", "web"]);

        dashboard.apply(&rebuild(
            "api",
            RebuildProgress::Running { step: 1, total: 2 },
        ));
        assert_eq!(
            dashboard.panes()[0].rebuild(),
            Some(&RebuildNotice::Running { step: 1, total: 2 })
        );
        assert_eq!(dashboard.panes()[1].rebuild(), None);

        dashboard.apply(&rebuild("api", RebuildProgress::Succeeded));
        assert_eq!(dashboard.panes()[0].rebuild(), None);
    }

    #[test]
    fn a_failed_rebuild_stays_visible_until_the_service_is_no_longer_live() {
        let mut dashboard = dashboard_for(&["api"]);
        let reason = "prepare step 1/1 failed with code 2".to_owned();
        let failed = RebuildNotice::Failed {
            reason: reason.clone(),
        };

        dashboard.apply(&rebuild("api", RebuildProgress::Failed { reason }));
        dashboard.apply(&Event::State {
            service: ServiceId::new("api"),
            state: ServiceState::Ready { pid: 1 },
        });
        assert_eq!(dashboard.panes()[0].rebuild(), Some(&failed));
        dashboard.apply(&Event::State {
            service: ServiceId::new("api"),
            state: ServiceState::Stopping,
        });

        assert_eq!(dashboard.panes()[0].rebuild(), None);
    }

    #[test]
    fn page_keys_scroll_back_within_the_capped_history() {
        let mut dashboard = dashboard_for(&["db"]);
        let states = running(&["db"]);
        for line in 0..SCROLLBACK_LINES + 100 {
            dashboard.apply(&output("db", &format!("line {line}\r\n")));
        }

        dashboard.on_key(key(KeyCode::PageUp), &states);
        assert_eq!(dashboard.selected_pane().scrolled(), 5);
        for _ in 0..1000 {
            dashboard.on_key(key(KeyCode::PageUp), &states);
        }
        assert_eq!(dashboard.selected_pane().scrolled(), SCROLLBACK_LINES);
        dashboard.on_key(key(KeyCode::PageDown), &states);
        assert_eq!(dashboard.selected_pane().scrolled(), SCROLLBACK_LINES - 5);
    }

    #[test]
    fn resize_resizes_every_pane_and_asks_the_supervisor_to_resize_ptys() {
        let mut dashboard = dashboard_for(&["db", "api"]);

        let command = dashboard.resize(PaneSize { rows: 30, cols: 90 });

        assert_eq!(command, Command::Resize { rows: 30, cols: 90 });
        assert!(
            dashboard
                .panes()
                .iter()
                .all(|pane| pane.screen().size() == (30, 90))
        );
    }
}
