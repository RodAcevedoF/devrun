use clap::Subcommand;

use crate::app::App;

#[derive(Subcommand)]
pub enum ProfileCommand {}

pub fn run(_app: &App, command: &ProfileCommand) -> anyhow::Result<()> {
    match *command {}
}
