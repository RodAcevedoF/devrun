use anyhow::bail;
use clap::Subcommand;
use devrun_domain::{ProfileName, RepoWorkspace, ServiceName, WorkspaceRepository};

use crate::app::App;
use crate::slices::init::prompter::{Choice, Prompter};
use crate::slices::init::terminal::{self, ClackPrompter};
use crate::ui::{self, Cell};

const NO_PICKER: &str =
    "no services given and no terminal for the picker; pass services explicitly";

#[derive(Subcommand)]
pub enum ProfileCommand {
    #[command(
        about = "Create or replace a named combination of services (interactive picker without services)"
    )]
    Set {
        #[arg(value_name = "PROFILE")]
        name: String,
        #[arg(value_name = "SERVICE", help = "Services or aliases in the profile")]
        members: Vec<String>,
    },
    #[command(about = "Remove a profile")]
    Rm {
        #[arg(value_name = "PROFILE")]
        name: String,
    },
    #[command(about = "List profiles")]
    Ls,
}

pub fn run(app: &App, command: &ProfileCommand) -> anyhow::Result<()> {
    let mut workspace = app.workspace()?;
    match command {
        ProfileCommand::Set { name, members } => {
            let name = parse_name(name)?;
            let members = if members.is_empty() {
                pick(&workspace, &name)?
            } else {
                resolve(&workspace, members)?
            };
            workspace.set_profile(name.clone(), members)?;
            app.store().save(&workspace)?;
            println!("profile @{name}: {}", members_of(&workspace, &name));
        }
        ProfileCommand::Rm { name } => {
            let name = parse_name(name)?;
            workspace.remove_profile(&name)?;
            app.store().save(&workspace)?;
            println!("removed profile @{name}");
        }
        ProfileCommand::Ls => print!("{}", render(&workspace)),
    }
    Ok(())
}

fn parse_name(raw: &str) -> anyhow::Result<ProfileName> {
    Ok(ProfileName::parse(raw.strip_prefix('@').unwrap_or(raw))?)
}

fn resolve(workspace: &RepoWorkspace, members: &[String]) -> anyhow::Result<Vec<ServiceName>> {
    members
        .iter()
        .map(|member| Ok(workspace.resolve_name(member)?.name.clone()))
        .collect()
}

fn pick(workspace: &RepoWorkspace, name: &ProfileName) -> anyhow::Result<Vec<ServiceName>> {
    if !terminal::is_interactive() {
        bail!(NO_PICKER);
    }
    let current = workspace
        .profiles()
        .get(name)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let services: Vec<ServiceName> = workspace
        .services()
        .map(|service| service.name.clone())
        .collect();
    let choices: Vec<Choice> = workspace
        .services()
        .map(|service| Choice {
            label: service.name.to_string(),
            hint: format!("{}  {}", service.runtime, service.path),
            checked: current.contains(&service.name),
        })
        .collect();
    let picked = terminal::or_exit_cancelled(ClackPrompter::default().multiselect(
        &format!("Services for @{name}"),
        &choices,
        true,
    ))?;
    Ok(picked
        .into_iter()
        .filter_map(|index| services.get(index).cloned())
        .collect())
}

fn members_of(workspace: &RepoWorkspace, name: &ProfileName) -> String {
    workspace
        .profiles()
        .get(name)
        .map(|members| joined(members))
        .unwrap_or_default()
}

fn joined(members: &[ServiceName]) -> String {
    members
        .iter()
        .map(ServiceName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn render(workspace: &RepoWorkspace) -> String {
    let rows: Vec<Vec<Cell>> = workspace
        .profiles()
        .iter()
        .map(|(profile, members)| {
            vec![
                Cell::new(format!("@{profile}"), ui::name_style()),
                Cell::plain(joined(members)),
            ]
        })
        .collect();
    let body = if rows.is_empty() {
        vec![ui::none()]
    } else {
        ui::table(&rows)
    };
    format!("{}\n{}\n", ui::heading("Profiles"), body.join("\n"))
}
