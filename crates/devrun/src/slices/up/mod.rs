use clap::Args;

use crate::app::App;

#[derive(Args)]
pub struct UpArgs {
    #[arg(
        value_name = "TARGET",
        allow_hyphen_values = true,
        help = "Services, aliases or @profiles; prefix with + to add or - to remove (comma lists allowed)"
    )]
    pub targets: Vec<String>,
}

pub fn run(_app: &App, _args: &UpArgs) -> anyhow::Result<()> {
    anyhow::bail!("`devrun up` is not implemented yet")
}
