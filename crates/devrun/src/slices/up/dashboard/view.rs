use devrun_supervisor::{ResourceUsage, ServiceState, ServiceStatus};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use tui_term::widget::{Cursor, PseudoTerminal};

use super::super::status::{self, Tone};
use super::model::{Dashboard, Mode, Pane, PaneSize, RebuildNotice};

pub const SIDEBAR_WIDTH: u16 = 30;
const BORDER: u16 = 2;
const MIN_PANE_ROWS: u16 = 4;
const MIN_PANE_COLS: u16 = 20;
const BADGE_WIDTH: usize = 9;
const BROWSE_KEYS: [(&str, &str); 7] = [
    ("↑↓", "select"),
    ("tab", "next"),
    ("r", "restart"),
    ("s", "stop/start"),
    ("i", "input"),
    ("pgup/pgdn", "scroll"),
    ("q", "quit"),
];
static WAITING: ServiceState = ServiceState::Waiting { on: Vec::new() };

struct Areas {
    header: Rect,
    sidebar: Rect,
    pane: Rect,
    help: Rect,
}

fn areas(area: Rect) -> Areas {
    let [header, body, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(BORDER + 1),
        Constraint::Length(1),
    ])
    .areas(area);
    let [sidebar, pane] = Layout::horizontal([
        Constraint::Length(SIDEBAR_WIDTH),
        Constraint::Min(BORDER + 1),
    ])
    .areas(body);
    Areas {
        header,
        sidebar,
        pane,
        help,
    }
}

pub fn pane_size(area: Rect) -> PaneSize {
    let pane = areas(area).pane;
    PaneSize {
        rows: pane.height.saturating_sub(BORDER).max(MIN_PANE_ROWS),
        cols: pane.width.saturating_sub(BORDER).max(MIN_PANE_COLS),
    }
}

pub fn draw(frame: &mut Frame, dashboard: &Dashboard, statuses: &[ServiceStatus]) {
    let areas = areas(frame.area());
    draw_header(frame, areas.header, dashboard, statuses);
    draw_sidebar(frame, areas.sidebar, dashboard, statuses);
    draw_pane(frame, areas.pane, dashboard, statuses);
    draw_help(frame, areas.help, dashboard);
}

fn state_of<'a>(pane: &Pane, statuses: &'a [ServiceStatus]) -> &'a ServiceState {
    statuses
        .iter()
        .find(|status| status.id == pane.id)
        .map_or(&WAITING, |status| &status.state)
}

fn tone_style(tone: Tone) -> Style {
    let style = Style::default();
    match tone {
        Tone::Idle => style.fg(Color::Gray),
        Tone::Preparing => style.fg(Color::Yellow),
        Tone::Active => style.fg(Color::Blue),
        Tone::Ready => style.fg(Color::Green),
        Tone::Failed => style.fg(Color::Red),
        Tone::Ended => style.add_modifier(Modifier::DIM),
    }
}

fn rebuild_span(notice: &RebuildNotice) -> Span<'static> {
    let (text, tone) = match notice {
        RebuildNotice::Running { step, total } => {
            (status::rebuilding(*step, *total), Tone::Preparing)
        }
        RebuildNotice::Failed { reason } => (status::rebuild_failed(reason), Tone::Failed),
    };
    Span::styled(format!(" · {text}"), tone_style(tone))
}

fn dim() -> Style {
    Style::default().add_modifier(Modifier::DIM)
}

fn bold() -> Style {
    Style::default().add_modifier(Modifier::BOLD)
}

fn ports_label(ports: &[u16]) -> String {
    ports
        .iter()
        .map(|port| format!(":{port}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn live_usage(pane: &Pane, state: &ServiceState) -> Option<ResourceUsage> {
    pane.usage().filter(|_| status::is_live(state))
}

fn usage_totals(dashboard: &Dashboard, statuses: &[ServiceStatus]) -> Option<String> {
    let live: Vec<ResourceUsage> = dashboard
        .panes()
        .iter()
        .filter_map(|pane| live_usage(pane, state_of(pane, statuses)))
        .collect();
    if live.is_empty() {
        return None;
    }
    let cpu = live
        .iter()
        .map(|usage| usage.cpu_permille)
        .fold(0, u32::saturating_add);
    let memory = live
        .iter()
        .map(|usage| usage.rss_bytes)
        .fold(0, u64::saturating_add);
    Some(format!(
        "{} cpu · {}",
        status::cpu_share(cpu),
        status::memory_size(memory)
    ))
}

fn draw_header(frame: &mut Frame, area: Rect, dashboard: &Dashboard, statuses: &[ServiceStatus]) {
    let title = Line::from(vec![
        Span::styled(" devrun", bold().fg(Color::Cyan)),
        Span::styled(" · ", dim()),
        Span::styled(dashboard.repo(), bold()),
    ]);
    let count = statuses.len();
    let mut summary = format!(
        "{count} {} · {} ready ",
        status::services_noun(count),
        status::ready_count(statuses)
    );
    if let Some(totals) = usage_totals(dashboard, statuses) {
        summary = format!("{summary}· {totals} ");
    }
    if dashboard.mode() == Mode::Stopping {
        summary = format!("stopping… · {summary}");
    }
    frame.render_widget(Paragraph::new(title), area);
    frame.render_widget(Paragraph::new(Line::from(summary).right_aligned()), area);
}

fn draw_sidebar(frame: &mut Frame, area: Rect, dashboard: &Dashboard, statuses: &[ServiceStatus]) {
    let items: Vec<ListItem> = dashboard
        .panes()
        .iter()
        .map(|pane| sidebar_item(pane, state_of(pane, statuses)))
        .collect();
    let list = List::new(items)
        .block(Block::bordered().title(" services ").border_style(dim()))
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");
    let mut state = ListState::default().with_selected(Some(dashboard.selected()));
    frame.render_stateful_widget(list, area, &mut state);
}

fn sidebar_item<'a>(pane: &'a Pane, state: &ServiceState) -> ListItem<'a> {
    let badge = format!("● {:<BADGE_WIDTH$}", status::badge(state));
    let mut spans = vec![
        Span::styled(badge, tone_style(status::tone(state))),
        Span::raw(" "),
        Span::raw(pane.id.as_str()),
        Span::raw(" "),
    ];
    if !pane.ports.is_empty() {
        spans.push(Span::styled(ports_label(&pane.ports), dim()));
    }
    ListItem::new(Line::from(spans))
}

fn draw_pane(frame: &mut Frame, area: Rect, dashboard: &Dashboard, statuses: &[ServiceStatus]) {
    let pane = dashboard.selected_pane();
    let state = state_of(pane, statuses);
    let typing = dashboard.mode() == Mode::Input;
    let mut block = Block::bordered()
        .title(pane_title(pane, state))
        .border_style(pane_border(state, typing));
    if pane.scrolled() > 0 {
        block = block.title_bottom(
            Line::styled(format!(" ↑ {} lines back ", pane.scrolled()), dim()).right_aligned(),
        );
    }
    let cursor = Cursor::default().visibility(typing && pane.scrolled() == 0);
    frame.render_widget(
        PseudoTerminal::new(pane.screen())
            .block(block)
            .cursor(cursor),
        area,
    );
}

fn pane_title<'a>(pane: &'a Pane, state: &ServiceState) -> Line<'a> {
    let tone = status::tone(state);
    let detail_style = if tone == Tone::Failed {
        tone_style(tone)
    } else {
        Style::default()
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            format!("{} · {}", pane.id, status::describe(state)),
            detail_style.add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(notice) = pane.rebuild() {
        spans.push(rebuild_span(notice));
    }
    if !pane.ports.is_empty() {
        spans.push(Span::styled(
            format!(" · {}", ports_label(&pane.ports)),
            dim(),
        ));
    }
    if let Some(usage) = live_usage(pane, state) {
        spans.push(Span::styled(
            format!(
                " · {} · {}",
                status::cpu_share(usage.cpu_permille),
                status::memory_size(usage.rss_bytes)
            ),
            dim(),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

fn pane_border(state: &ServiceState, typing: bool) -> Style {
    if typing {
        Style::default().fg(Color::Yellow)
    } else if status::tone(state) == Tone::Failed {
        Style::default().fg(Color::Red)
    } else {
        dim()
    }
}

fn draw_help(frame: &mut Frame, area: Rect, dashboard: &Dashboard) {
    let line = match dashboard.mode() {
        Mode::Browse => key_help(&BROWSE_KEYS),
        Mode::Input => {
            let mut line = Line::from(vec![
                Span::styled(" typing into ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    dashboard.selected_pane().id.as_str(),
                    bold().fg(Color::Yellow),
                ),
                Span::raw("  "),
            ]);
            line.spans.extend(key_help(&[("esc", "leave")]).spans);
            line
        }
        Mode::Stopping => Line::styled(
            " stopping… waiting for every service to exit",
            Style::default().fg(Color::Yellow),
        ),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn key_help(keys: &[(&'static str, &'static str)]) -> Line<'static> {
    let spans = keys.iter().flat_map(|(key, action)| {
        [
            Span::styled(format!(" {key}"), bold()),
            Span::styled(format!(" {action} "), dim()),
        ]
    });
    Line::from(spans.collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use devrun_supervisor::{Event, OutputSource, ResourceUsage, ServiceId, ServiceState};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};

    use super::super::model::tests::{dashboard_for, statuses};
    use super::*;

    const WIDTH: u16 = 100;
    const HEIGHT: u16 = 16;

    fn fixed_states() -> Vec<ServiceStatus> {
        statuses(&[
            ("db", ServiceState::Waiting { on: Vec::new() }),
            ("deps", ServiceState::Preparing { step: 1, total: 2 }),
            ("api", ServiceState::Running { pid: 7 }),
            ("web", ServiceState::Ready { pid: 8 }),
            (
                "kg",
                ServiceState::Failed {
                    reason: "prepare step 1/1 failed with code 4".to_owned(),
                },
            ),
            ("job", ServiceState::Exited { code: Some(0) }),
        ])
    }

    fn fixed_dashboard() -> Dashboard {
        dashboard_for(&["db", "deps", "api", "web", "kg", "job"])
    }

    fn render(dashboard: &Dashboard, states: &[ServiceStatus]) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
        terminal
            .draw(|frame| draw(frame, dashboard, states))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buffer: &Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn find(buffer: &Buffer, needle: &str) -> (u16, u16) {
        rows(buffer)
            .iter()
            .enumerate()
            .find_map(|(y, row)| {
                row.find(needle).map(|byte| {
                    let x = row[..byte].chars().count();
                    (u16::try_from(x).unwrap(), u16::try_from(y).unwrap())
                })
            })
            .unwrap_or_else(|| panic!("`{needle}` not rendered:\n{}", rows(buffer).join("\n")))
    }

    fn sidebar_row(buffer: &Buffer, name: &str) -> String {
        rows(buffer)
            .into_iter()
            .find(|row| row.contains(&format!(" {name} ")) && row.contains('●'))
            .unwrap_or_else(|| panic!("no sidebar row for {name}"))
    }

    fn select(dashboard: &mut Dashboard, states: &[ServiceStatus], name: &str) {
        for _ in 0..dashboard.panes().len() {
            if dashboard.selected_pane().id == ServiceId::new(name) {
                return;
            }
            dashboard.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), states);
        }
        panic!("{name} cannot be selected");
    }

    const MIB: u64 = 1024 * 1024;

    fn usage(service: &str, cpu_permille: u32, rss_bytes: u64) -> Event {
        Event::Usage {
            service: ServiceId::new(service),
            usage: ResourceUsage {
                cpu_permille,
                rss_bytes,
                processes: 1,
            },
        }
    }

    fn measured_dashboard() -> Dashboard {
        let mut dashboard = fixed_dashboard();
        dashboard.apply(&usage("api", 125, 150 * MIB));
        dashboard.apply(&usage("web", 75, 50 * MIB));
        dashboard.apply(&usage("job", 900, 1024 * MIB));
        dashboard
    }

    #[test]
    fn header_names_the_repo_and_counts_ready_services() {
        let buffer = render(&fixed_dashboard(), &fixed_states());

        let header = &rows(&buffer)[0];
        assert!(header.contains("devrun · jaspy-monorepo"), "{header}");
        assert!(header.contains("6 services · 1 ready "), "{header}");
        assert!(!header.contains("cpu"), "{header}");
    }

    #[test]
    fn header_totals_usage_over_live_services_only() {
        let buffer = render(&measured_dashboard(), &fixed_states());

        let header = &rows(&buffer)[0];
        assert!(
            header.contains("6 services · 1 ready · 20.0% cpu · 200 MB "),
            "{header}"
        );
    }

    #[test]
    fn pane_title_shows_the_usage_of_a_live_service() {
        let mut dashboard = measured_dashboard();
        let states = fixed_states();
        select(&mut dashboard, &states, "api");

        let buffer = render(&dashboard, &states);

        find(&buffer, "api · running · 12.5% · 150 MB ");
    }

    #[test]
    fn pane_title_shows_rebuild_progress_after_the_state() {
        let mut dashboard = fixed_dashboard();
        let states = fixed_states();
        select(&mut dashboard, &states, "api");
        dashboard.apply(&Event::Rebuild {
            service: ServiceId::new("api"),
            progress: devrun_supervisor::RebuildProgress::Running { step: 1, total: 2 },
        });

        let buffer = render(&dashboard, &states);

        find(&buffer, "api · running · rebuilding 1/2 ");
    }

    #[test]
    fn pane_title_shows_a_failed_rebuild_in_the_failure_color() {
        let mut dashboard = fixed_dashboard();
        let states = fixed_states();
        select(&mut dashboard, &states, "api");
        dashboard.apply(&Event::Rebuild {
            service: ServiceId::new("api"),
            progress: devrun_supervisor::RebuildProgress::Failed {
                reason: "prepare step 1/1 failed with code 2".to_owned(),
            },
        });

        let buffer = render(&dashboard, &states);

        let (x, y) = find(
            &buffer,
            "rebuild failed: prepare step 1/1 failed with code 2",
        );
        assert_eq!(buffer[(x, y)].fg, Color::Red);
    }

    #[test]
    fn pane_title_shows_ports_before_usage() {
        let mut dashboard = Dashboard::new(
            "repo".to_owned(),
            &[devrun_supervisor::LaunchSpec {
                ports: vec![3000],
                ..devrun_supervisor::LaunchSpec::new(ServiceId::new("api"), "/repo", "nest")
            }],
            super::super::model::tests::sample_size(),
        );
        dashboard.apply(&usage("api", 125, 150 * MIB));

        let buffer = render(
            &dashboard,
            &statuses(&[("api", ServiceState::Running { pid: 1 })]),
        );

        find(&buffer, "api · running · :3000 · 12.5% · 150 MB ");
    }

    #[test]
    fn pane_title_hides_the_usage_of_an_ended_service() {
        let mut dashboard = measured_dashboard();
        let states = fixed_states();
        select(&mut dashboard, &states, "job");

        let buffer = render(&dashboard, &states);

        let (_, y) = find(&buffer, "job · exited with code 0");
        let title = &rows(&buffer)[usize::from(y)];
        assert!(!title.contains('%'), "{title}");
        assert!(!title.contains("GB"), "{title}");
    }

    #[test]
    fn sidebar_shows_a_colored_badge_per_service_state() {
        let buffer = render(&fixed_dashboard(), &fixed_states());
        let expected = [
            ("db", "waiting", Color::Gray),
            ("deps", "preparing", Color::Yellow),
            ("api", "running", Color::Blue),
            ("web", "ready", Color::Green),
            ("kg", "failed", Color::Red),
        ];

        for (name, badge, color) in expected {
            let row = sidebar_row(&buffer, name);
            assert!(row.contains(badge), "{row}");
            let (x, y) = find(&buffer, &format!("● {badge}"));
            assert_eq!(buffer[(x, y)].fg, color, "{name}");
        }
        let (x, y) = find(&buffer, "● exited");
        assert!(buffer[(x, y)].modifier.contains(Modifier::DIM));
    }

    #[test]
    fn sidebar_lists_ports_next_to_the_name() {
        let mut dashboard = dashboard_for(&["web"]);
        dashboard = Dashboard::new(
            dashboard.repo().to_owned(),
            &[devrun_supervisor::LaunchSpec {
                ports: vec![5173],
                ..devrun_supervisor::LaunchSpec::new(ServiceId::new("web"), "/repo", "vite")
            }],
            super::super::model::tests::sample_size(),
        );

        let buffer = render(
            &dashboard,
            &statuses(&[("web", ServiceState::Ready { pid: 1 })]),
        );

        assert!(sidebar_row(&buffer, "web").contains(":5173"));
    }

    #[test]
    fn pane_header_shows_the_selected_service_and_its_failure_reason() {
        let mut dashboard = fixed_dashboard();
        let states = fixed_states();
        select(&mut dashboard, &states, "kg");

        let buffer = render(&dashboard, &states);

        let (x, y) = find(&buffer, "kg · failed: prepare step 1/1 failed with code 4");
        assert_eq!(buffer[(x, y)].fg, Color::Red);
    }

    #[test]
    fn pane_renders_the_selected_service_terminal() {
        let mut dashboard = fixed_dashboard();
        dashboard.apply(&Event::Output {
            service: ServiceId::new("db"),
            source: OutputSource::Service,
            bytes: b"database system is ready\r\n".to_vec(),
        });

        let buffer = render(&dashboard, &fixed_states());

        find(&buffer, "database system is ready");
    }

    #[test]
    fn help_bar_lists_the_keys_for_each_mode() {
        let mut dashboard = fixed_dashboard();
        let states = fixed_states();

        let browse = rows(&render(&dashboard, &states)).pop().unwrap();
        dashboard.on_key(
            KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE),
            &states,
        );
        let input = rows(&render(&dashboard, &states)).pop().unwrap();
        dashboard.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &states);
        dashboard.on_key(
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE),
            &states,
        );
        let stopping = rows(&render(&dashboard, &states)).pop().unwrap();

        for keys in [
            "↑↓ select",
            "tab next",
            "r restart",
            "s stop/start",
            "i input",
            "pgup/pgdn scroll",
            "q quit",
        ] {
            assert!(browse.contains(keys), "{browse}");
        }
        assert!(input.contains("typing into db"), "{input}");
        assert!(input.contains("esc leave"), "{input}");
        assert!(stopping.contains("stopping…"), "{stopping}");
    }

    #[test]
    fn a_zero_sized_terminal_still_gets_a_pane_that_accepts_output() {
        let size = pane_size(Rect::new(0, 0, 0, 0));
        let specs = [devrun_supervisor::LaunchSpec::new(
            ServiceId::new("web"),
            "/repo",
            "vite",
        )];
        let mut dashboard = Dashboard::new("repo".to_owned(), &specs, size);

        dashboard.apply(&Event::Output {
            service: ServiceId::new("web"),
            source: OutputSource::Service,
            bytes: b"hello-dashboard\r\nnext line\r\n".to_vec(),
        });

        assert!(size.rows >= 2 && size.cols >= 2, "{size:?}");
    }

    #[test]
    fn pane_size_matches_the_terminal_area_inside_the_pane_border() {
        let size = pane_size(Rect::new(0, 0, WIDTH, HEIGHT));

        assert_eq!(size.rows, HEIGHT - 4);
        assert_eq!(size.cols, WIDTH - SIDEBAR_WIDTH - 2);
    }
}
