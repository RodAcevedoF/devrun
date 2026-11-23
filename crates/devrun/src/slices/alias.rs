use clap::Subcommand;
use devrun_domain::{AliasName, RepoWorkspace, WorkspaceRepository};

use crate::app::App;
use crate::ui::{self, Cell};

#[derive(Subcommand)]
pub enum AliasCommand {
    #[command(about = "Create or replace a short name for a service")]
    Set {
        #[arg(value_name = "ALIAS")]
        alias: String,
        #[arg(value_name = "SERVICE", help = "Service or existing alias to point to")]
        service: String,
    },
    #[command(about = "Remove an alias")]
    Rm {
        #[arg(value_name = "ALIAS")]
        alias: String,
    },
    #[command(about = "List aliases")]
    Ls,
}

pub fn run(app: &App, command: &AliasCommand) -> anyhow::Result<()> {
    let mut workspace = app.workspace()?;
    match command {
        AliasCommand::Set { alias, service } => {
            let alias = AliasName::parse(alias)?;
            let target = workspace.resolve_name(service)?.name.clone();
            workspace.set_alias(alias.clone(), target.clone())?;
            app.store().save(&workspace)?;
            println!("alias {alias} -> {target}");
        }
        AliasCommand::Rm { alias } => {
            let alias = AliasName::parse(alias)?;
            workspace.remove_alias(&alias)?;
            app.store().save(&workspace)?;
            println!("removed alias {alias}");
        }
        AliasCommand::Ls => print!("{}", render(&workspace)),
    }
    Ok(())
}

fn render(workspace: &RepoWorkspace) -> String {
    let rows: Vec<Vec<Cell>> = workspace
        .aliases()
        .iter()
        .map(|(alias, target)| {
            vec![
                Cell::new(alias.as_str(), ui::name_style()),
                Cell::plain(format!("-> {target}")),
            ]
        })
        .collect();
    let body = if rows.is_empty() {
        vec![ui::none()]
    } else {
        ui::table(&rows)
    };
    format!("{}\n{}\n", ui::heading("Aliases"), body.join("\n"))
}
