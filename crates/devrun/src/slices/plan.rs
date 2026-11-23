use devrun_domain::{PlanReason, RunPlan, TargetExpr};

use crate::app::App;
use crate::ui::{self, Cell};

pub fn show(app: &App, targets: &[String]) -> anyhow::Result<()> {
    let workspace = app.workspace()?;
    let plan = RunPlan::resolve(&workspace, &TargetExpr::parse(targets)?)?;
    for skipped in plan.skipped_needs() {
        ui::print_warning(&format!(
            "{} needs {}, which was excluded",
            skipped.service, skipped.need
        ));
    }
    println!("{}", render(&plan));
    Ok(())
}

fn render(plan: &RunPlan) -> String {
    let count = plan.services().len();
    let noun = if count == 1 { "service" } else { "services" };
    let digits = count.to_string().len();
    let rows: Vec<Vec<Cell>> = plan
        .services()
        .iter()
        .enumerate()
        .map(|(index, planned)| {
            let reason = match &planned.reason {
                PlanReason::Requested => "requested".to_owned(),
                PlanReason::NeededBy(service) => format!("needed by {service}"),
            };
            vec![
                Cell::new(
                    format!("{:>digits$}. {}", index + 1, planned.service.name),
                    ui::name_style(),
                ),
                Cell::new(planned.service.runtime.as_str(), ui::dim_style()),
                Cell::plain(reason),
            ]
        })
        .collect();
    let mut lines = vec![ui::heading(&format!("Plan: {count} {noun}"))];
    lines.extend(ui::table(&rows));
    lines.join("\n")
}
