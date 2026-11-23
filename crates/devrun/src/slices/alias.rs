use clap::Subcommand;

use crate::app::App;

#[derive(Subcommand)]
pub enum AliasCommand {}

pub fn run(_app: &App, command: &AliasCommand) -> anyhow::Result<()> {
    match *command {}
}
