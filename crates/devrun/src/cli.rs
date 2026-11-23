use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use crate::app::App;
use crate::slices::{alias, catalog, config, discover, init, plan, profile, up};
use crate::ui;

#[derive(Parser)]
#[command(
    name = "devrun",
    version,
    about = "Launch only the dev servers you need from your monorepo"
)]
struct Cli {
    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Repository to work on (defaults to the git repository around the current directory)"
    )]
    repo: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "List configured services, aliases and profiles")]
    Ls,
    #[command(about = "Show what would start, in order, without starting anything")]
    Plan(TargetArgs),
    #[command(about = "Scan the repository for services and compare them with the config")]
    Discover,
    #[command(
        about = "Discover the repository and choose services, aliases and profiles interactively"
    )]
    Init(init::InitArgs),
    #[command(
        about = "Start services, aliases or @profiles in the dashboard (interactive picker without targets)"
    )]
    Up(up::UpArgs),
    #[command(subcommand, about = "Manage short names for services")]
    Alias(alias::AliasCommand),
    #[command(subcommand, about = "Manage named combinations of services")]
    Profile(profile::ProfileCommand),
    #[command(subcommand, about = "Inspect devrun's own configuration")]
    Config(ConfigCommand),
}

#[derive(Subcommand)]
enum ConfigCommand {
    #[command(about = "Print the config file path for this repository")]
    Path,
}

#[derive(Args)]
pub struct TargetArgs {
    #[arg(
        value_name = "TARGET",
        required = true,
        num_args = 1..,
        allow_hyphen_values = true,
        help = "Services, aliases or @profiles; prefix with + to add or - to remove (comma lists allowed)"
    )]
    pub targets: Vec<String>,
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            ui::print_error(&error);
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: Cli) -> anyhow::Result<()> {
    let app = App::open(cli.repo.as_deref())?;
    match cli.command {
        Command::Ls => catalog::list(&app),
        Command::Plan(args) => plan::show(&app, &args.targets),
        Command::Discover => discover::show(&app),
        Command::Init(args) => init::run(&app, &args),
        Command::Up(args) => up::run(&app, &args),
        Command::Alias(command) => alias::run(&app, &command),
        Command::Profile(command) => profile::run(&app, &command),
        Command::Config(ConfigCommand::Path) => {
            config::path(&app);
            Ok(())
        }
    }
}
