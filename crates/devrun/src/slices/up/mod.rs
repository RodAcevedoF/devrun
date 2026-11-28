mod dashboard;
mod headless;
mod launch;
mod lines;
mod picker;
mod preflight;
mod status;
mod watch;

use std::io::{self, IsTerminal};

use anyhow::Context;
use clap::Args;
use devrun_domain::{RunPlan, TargetExpr};
use devrun_supervisor::TerminationSignals;

use watch::{Watch, WatchTargets};

use crate::app::App;
use crate::ui;

const RUNTIME_WORKERS: usize = 2;
const NO_TUI: &str = "no-tui";
const NO_WATCH: &str = "no-watch";
const SWITCHES: [&str; 2] = [NO_TUI, NO_WATCH];

#[derive(Args)]
pub struct UpArgs {
    #[arg(
        value_name = "TARGET",
        allow_hyphen_values = true,
        help = "Services, aliases or @profiles; prefix with + to add or - to remove (comma lists allowed)"
    )]
    pub targets: Vec<String>,
    #[arg(
        long = NO_TUI,
        help = "Stream prefixed logs instead of opening the dashboard (automatic when stdout is not a terminal)"
    )]
    pub no_tui: bool,
    #[arg(
        long = NO_WATCH,
        help = "Do not rebuild services when files in their watch dirs change"
    )]
    pub no_watch: bool,
}

struct Switches {
    no_tui: bool,
    no_watch: bool,
}

#[derive(Clone, Copy)]
enum View {
    Dashboard,
    Headless,
}

pub fn run(app: &App, args: &UpArgs) -> anyhow::Result<()> {
    let workspace = app.workspace()?;
    let (targets, switches) = split_switches(args);
    let targets = if targets.is_empty() {
        picker::choose_targets(&workspace, &app.repo_label())?
    } else {
        targets
    };
    let plan = RunPlan::resolve(&workspace, &TargetExpr::parse(&targets)?)?;
    warn_about_skipped_needs(&plan);
    let specs = launch::launch_specs(&plan, app.root());
    preflight::ensure_ports_free(&specs)?;
    let view = choose_view(switches.no_tui);
    let interrupts = TerminationSignals::block()?;
    let watch = if switches.no_watch {
        None
    } else {
        Watch::start(WatchTargets::from_plan(&plan, app.root())).with_context(|| {
            format!("cannot watch the services' watch dirs (pass --{NO_WATCH} to run without it)")
        })?
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKERS)
        .enable_all()
        .build()?;
    let repo_label = app.repo_label();
    runtime.block_on(async move {
        let interrupts = interrupts.listen();
        match view {
            View::Headless => headless::run(specs, watch, interrupts).await,
            View::Dashboard => dashboard::run(repo_label, specs, watch, interrupts).await,
        }
    })
}

fn split_switches(args: &UpArgs) -> (Vec<String>, Switches) {
    let (switches, targets): (Vec<String>, Vec<String>) = args
        .targets
        .iter()
        .cloned()
        .partition(|target| switch_name(target).is_some());
    let given = |wanted: &str| {
        switches
            .iter()
            .any(|switch| switch_name(switch) == Some(wanted))
    };
    let switches = Switches {
        no_tui: args.no_tui || given(NO_TUI),
        no_watch: args.no_watch || given(NO_WATCH),
    };
    (targets, switches)
}

fn switch_name(target: &str) -> Option<&str> {
    target
        .strip_prefix("--")
        .filter(|name| SWITCHES.contains(name))
}

fn warn_about_skipped_needs(plan: &RunPlan) {
    for skipped in plan.skipped_needs() {
        ui::print_warning(&format!(
            "{} needs {}, which was excluded",
            skipped.service, skipped.need
        ));
    }
}

fn choose_view(no_tui: bool) -> View {
    if no_tui || !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        View::Headless
    } else {
        View::Dashboard
    }
}
