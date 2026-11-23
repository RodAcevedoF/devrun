mod defaults;
pub mod prompter;
mod selection;
pub mod terminal;
mod wizard;

#[cfg(test)]
mod tests;

use anyhow::bail;
use clap::Args;

use crate::adapters::detectors;
use crate::app::App;
use terminal::ClackPrompter;
use wizard::{Outcome, Repo};

const NEEDS_TERMINAL: &str =
    "devrun init needs a terminal; pass --yes to accept the discovered defaults";

#[derive(Args)]
pub struct InitArgs {
    #[arg(
        long,
        short = 'y',
        help = "Accept the discovered high and medium confidence services and suggested aliases without prompting"
    )]
    yes: bool,
}

pub fn run(app: &App, args: &InitArgs) -> anyhow::Result<()> {
    if args.yes {
        return defaults::run(app);
    }
    if !terminal::is_interactive() {
        bail!(NEEDS_TERMINAL);
    }
    let repo = Repo {
        root: app.root().to_path_buf(),
        label: app.repo_label(),
        config_path: app.store().path_for(app.root()),
    };
    let mut prompter = ClackPrompter::default();
    match wizard::run(&mut prompter, app.store(), &repo, detectors::detect_all)? {
        Outcome::Saved => Ok(()),
        Outcome::Cancelled => terminal::exit_failure(),
    }
}
