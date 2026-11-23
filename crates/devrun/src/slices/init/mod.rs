use clap::Args;

use crate::app::App;

#[derive(Args)]
pub struct InitArgs {}

pub fn run(_app: &App, _args: &InitArgs) -> anyhow::Result<()> {
    anyhow::bail!("`devrun init` is not implemented yet")
}
