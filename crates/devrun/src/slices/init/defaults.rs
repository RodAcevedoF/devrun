use devrun_domain::{RepoWorkspace, Service, ServiceName, WorkspaceRepository};

use super::selection::{
    accepted_by_default, adopt, counted, is_new, joined, next_step, port_claims, suggest_aliases,
};
use crate::adapters::detectors;
use crate::app::App;
use crate::ui;

pub fn run(app: &App) -> anyhow::Result<()> {
    let mut workspace = match app.store().load(app.root())? {
        Some(workspace) => workspace,
        None => RepoWorkspace::new(app.root().to_path_buf())?,
    };
    let discovery = detectors::detect_all(app.root())?;
    for warning in &discovery.warnings {
        ui::print_warning(warning);
    }
    let incoming: Vec<Service> = discovery
        .candidates
        .iter()
        .filter(|candidate| accepted_by_default(candidate) && is_new(&workspace, candidate))
        .map(|candidate| candidate.service.clone())
        .collect();
    let names: Vec<ServiceName> = incoming
        .iter()
        .map(|service| service.name.clone())
        .collect();
    for problem in adopt(&mut workspace, incoming) {
        ui::print_warning(&problem);
    }
    let added: Vec<ServiceName> = names
        .into_iter()
        .filter(|name| workspace.service(name).is_some())
        .collect();
    for (port, claimants) in port_claims(workspace.services()) {
        ui::print_warning(&format!("port {port} is claimed by {}", joined(&claimants)));
    }
    let aliases = suggest_aliases(&workspace, &added);
    for (alias, target) in aliases.iter().cloned() {
        workspace.set_alias(alias, target)?;
    }
    app.store().save(&workspace)?;
    println!(
        "Added {} and {}",
        counted(added.len(), "service", "services"),
        counted(aliases.len(), "alias", "aliases")
    );
    println!("Saved {}", app.store().path_for(app.root()).display());
    if let Some(step) = next_step(&workspace) {
        println!("Next: {step}");
    }
    Ok(())
}
