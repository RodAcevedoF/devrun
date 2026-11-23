use devrun_domain::{
    Candidate, Change, DiscoveryDiff, RepoWorkspace, ServiceName, WorkspaceRepository, shared_ports,
};

use crate::adapters::detectors::{self, Discovery};
use crate::app::App;
use crate::ui::{self, Cell};

const INIT_HINT: &str = "run `devrun init` to choose which services to keep";
const DETAIL_INDENT: &str = "    ";

pub fn show(app: &App) -> anyhow::Result<()> {
    let discovery = detectors::detect_all(app.root())?;
    warn(&discovery);
    let mut lines = vec![
        ui::heading(&summary(discovery.candidates.len())),
        String::new(),
    ];
    match app.store().load(app.root())? {
        None => lines.extend(first_discovery(&discovery.candidates)),
        Some(workspace) => lines.extend(comparison(
            &workspace,
            &DiscoveryDiff::between(&workspace, &discovery.candidates),
        )),
    }
    println!("{}", lines.join("\n"));
    Ok(())
}

fn warn(discovery: &Discovery) {
    for warning in &discovery.warnings {
        ui::print_warning(warning);
    }
    for shared in shared_ports(&discovery.candidates) {
        ui::print_warning(&format!(
            "port {} is claimed by {}",
            shared.port,
            joined(&shared.services)
        ));
    }
}

fn summary(count: usize) -> String {
    let noun = if count == 1 { "service" } else { "services" };
    format!("Discovery: {count} {noun}")
}

fn first_discovery(candidates: &[Candidate]) -> Vec<String> {
    let mut lines = vec![ui::heading("New")];
    if candidates.is_empty() {
        lines.push(ui::none());
    } else {
        lines.extend(candidate_lines(candidates));
    }
    lines.push(String::new());
    lines.push(ui::dim_style().apply_to(INIT_HINT).to_string());
    lines
}

fn comparison(workspace: &RepoWorkspace, diff: &DiscoveryDiff) -> Vec<String> {
    if diff.is_empty() {
        return vec!["Up to date".to_owned()];
    }
    let sections = [
        ("New", candidate_lines(&diff.added)),
        ("Changed", change_lines(&diff.changed)),
        ("Missing", missing_lines(workspace, &diff.missing)),
    ];
    let mut lines = Vec::new();
    for (title, body) in sections.into_iter().filter(|(_, body)| !body.is_empty()) {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push(ui::heading(title));
        lines.extend(body);
    }
    lines
}

fn candidate_lines(candidates: &[Candidate]) -> Vec<String> {
    let with_ports = candidates
        .iter()
        .any(|candidate| !candidate.service.ports.is_empty());
    let rows: Vec<Vec<Cell>> = candidates
        .iter()
        .map(|candidate| candidate_row(candidate, with_ports))
        .collect();
    interleave(ui::table(&rows), candidates.iter().map(candidate_details))
}

fn candidate_row(candidate: &Candidate, with_ports: bool) -> Vec<Cell> {
    let service = &candidate.service;
    let mut row = vec![
        Cell::new(service.name.as_str(), ui::name_style()),
        Cell::new(service.runtime.as_str(), ui::dim_style()),
        Cell::plain(service.path.to_string()),
    ];
    if with_ports {
        let ports: Vec<String> = service
            .ports
            .iter()
            .map(|port| format!(":{port}"))
            .collect();
        row.push(Cell::plain(ports.join(" ")));
    }
    row.push(Cell::new(candidate.confidence.as_str(), ui::dim_style()));
    row.push(Cell::plain(service.command.as_str()));
    row
}

fn candidate_details(candidate: &Candidate) -> Vec<String> {
    let mut details = vec![why(candidate)];
    details.extend(
        candidate
            .service
            .prepare
            .iter()
            .map(|command| detail(&format!("prepare: {command}"))),
    );
    details
}

fn change_lines(changes: &[Change]) -> Vec<String> {
    let rows: Vec<Vec<Cell>> = changes
        .iter()
        .map(|change| {
            vec![
                Cell::new(change.current.name.as_str(), ui::name_style()),
                Cell::new(change.candidate.confidence.as_str(), ui::dim_style()),
            ]
        })
        .collect();
    interleave(ui::table(&rows), changes.iter().map(change_details))
}

fn change_details(change: &Change) -> Vec<String> {
    let (current, discovered) = (&change.current, &change.candidate.service);
    let differences = [
        (
            change.runtime_changed(),
            "runtime",
            current.runtime.to_string(),
            discovered.runtime.to_string(),
        ),
        (
            change.path_changed(),
            "path",
            current.path.to_string(),
            discovered.path.to_string(),
        ),
        (
            change.command_changed(),
            "command",
            current.command.to_string(),
            discovered.command.to_string(),
        ),
    ];
    let mut details: Vec<String> = differences
        .into_iter()
        .filter(|(changed, ..)| *changed)
        .map(|(_, field, from, to)| detail(&format!("{field}: {from} -> {to}")))
        .collect();
    details.push(why(&change.candidate));
    details
}

fn missing_lines(workspace: &RepoWorkspace, missing: &[ServiceName]) -> Vec<String> {
    let rows: Vec<Vec<Cell>> = missing
        .iter()
        .filter_map(|name| workspace.service(name))
        .map(|service| {
            vec![
                Cell::new(service.name.as_str(), ui::name_style()),
                Cell::new(service.runtime.as_str(), ui::dim_style()),
                Cell::plain(service.path.to_string()),
                Cell::plain(service.command.as_str()),
            ]
        })
        .collect();
    ui::table(&rows)
}

fn why(candidate: &Candidate) -> String {
    format!(
        "{DETAIL_INDENT}{}",
        ui::dim_style().apply_to(format!("why: {}", candidate.evidence))
    )
}

fn detail(text: &str) -> String {
    format!("{DETAIL_INDENT}{text}")
}

fn interleave(rows: Vec<String>, details: impl Iterator<Item = Vec<String>>) -> Vec<String> {
    rows.into_iter()
        .zip(details)
        .flat_map(|(row, details)| std::iter::once(row).chain(details))
        .collect()
}

fn joined(names: &[ServiceName]) -> String {
    names
        .iter()
        .map(ServiceName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}
