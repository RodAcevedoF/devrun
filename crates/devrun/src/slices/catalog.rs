use devrun_domain::{RepoWorkspace, Service};

use crate::app::App;
use crate::ui::{self, Cell};

pub fn list(app: &App) -> anyhow::Result<()> {
    let workspace = app.workspace()?;
    print!("{}", render(&app.repo_label(), &workspace));
    Ok(())
}

fn render(label: &str, workspace: &RepoWorkspace) -> String {
    let services: Vec<Vec<Cell>> = workspace.services().map(service_row).collect();
    let aliases: Vec<Vec<Cell>> = workspace
        .aliases()
        .iter()
        .map(|(alias, target)| {
            vec![
                Cell::new(alias.as_str(), ui::name_style()),
                Cell::plain(format!("-> {target}")),
            ]
        })
        .collect();
    let profiles: Vec<Vec<Cell>> = workspace
        .profiles()
        .iter()
        .map(|(profile, members)| {
            let members: Vec<&str> = members.iter().map(AsRef::as_ref).collect();
            vec![
                Cell::new(format!("@{profile}"), ui::name_style()),
                Cell::plain(members.join(", ")),
            ]
        })
        .collect();
    let mut lines = vec![format!(
        "{}  {}",
        ui::heading(label),
        workspace.root().display()
    )];
    for (title, rows) in [
        ("Services", services),
        ("Aliases", aliases),
        ("Profiles", profiles),
    ] {
        lines.push(String::new());
        lines.push(ui::heading(title));
        lines.extend(section(&rows));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn section(rows: &[Vec<Cell>]) -> Vec<String> {
    if rows.is_empty() {
        vec![ui::none()]
    } else {
        ui::table(rows)
    }
}

fn service_row(service: &Service) -> Vec<Cell> {
    let ports: Vec<String> = service
        .ports
        .iter()
        .map(|port| format!(":{port}"))
        .collect();
    let needs: Vec<&str> = service.needs.iter().map(AsRef::as_ref).collect();
    vec![
        Cell::new(service.name.as_str(), ui::name_style()),
        Cell::new(service.runtime.as_str(), ui::dim_style()),
        Cell::plain(service.path.to_string()),
        Cell::plain(ports.join(" ")),
        Cell::new(
            if needs.is_empty() {
                String::new()
            } else {
                format!("needs {}", needs.join(", "))
            },
            ui::dim_style(),
        ),
    ]
}
