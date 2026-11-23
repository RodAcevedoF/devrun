mod dashboard;
mod headless;
mod launch;
mod lines;
mod picker;
mod preflight;
mod status;

use std::io::{self, IsTerminal};

use clap::Args;
use devrun_domain::{RunPlan, TargetExpr};
use devrun_supervisor::TerminationSignals;

use crate::app::App;
use crate::ui;

const RUNTIME_WORKERS: usize = 2;
const NO_TUI_FLAG: &str = "--no-tui";

#[derive(Args)]
pub struct UpArgs {
    #[arg(
        value_name = "TARGET",
        allow_hyphen_values = true,
        help = "Services, aliases or @profiles; prefix with + to add or - to remove (comma lists allowed)"
    )]
    pub targets: Vec<String>,
    #[arg(
        long,
        help = "Stream prefixed logs instead of opening the dashboard (automatic when stdout is not a terminal)"
    )]
    pub no_tui: bool,
}

#[derive(Clone, Copy)]
enum View {
    Dashboard,
    Headless,
}

pub fn run(app: &App, args: &UpArgs) -> anyhow::Result<()> {
    let workspace = app.workspace()?;
    let (targets, no_tui) = split_no_tui_flag(args);
    let targets = if targets.is_empty() {
        picker::choose_targets(&workspace, &app.repo_label())?
    } else {
        targets
    };
    let plan = RunPlan::resolve(&workspace, &TargetExpr::parse(&targets)?)?;
    warn_about_skipped_needs(&plan);
    let specs = launch::launch_specs(&plan, app.root());
    preflight::ensure_ports_free(&specs)?;
    let view = choose_view(no_tui);
    let interrupts = TerminationSignals::block()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_WORKERS)
        .enable_all()
        .build()?;
    let repo_label = app.repo_label();
    runtime.block_on(async move {
        let interrupts = interrupts.listen();
        match view {
            View::Headless => headless::run(specs, interrupts).await,
            View::Dashboard => dashboard::run(repo_label, specs, interrupts).await,
        }
    })
}

fn split_no_tui_flag(args: &UpArgs) -> (Vec<String>, bool) {
    let (flags, targets): (Vec<String>, Vec<String>) = args
        .targets
        .iter()
        .cloned()
        .partition(|target| target == NO_TUI_FLAG);
    (targets, args.no_tui || !flags.is_empty())
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
