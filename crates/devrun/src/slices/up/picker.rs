use std::io::{self, IsTerminal};

use anyhow::{anyhow, bail};
use devrun_domain::{RepoWorkspace, Service, ServiceName};

pub fn choose_targets(workspace: &RepoWorkspace, repo_label: &str) -> anyhow::Result<Vec<String>> {
    if !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        bail!(
            "no targets given and no terminal for the picker; pass services, aliases or @profiles"
        );
    }
    cliclack::intro(format!("devrun · {repo_label}"))?;
    let mut prompt = cliclack::multiselect("What should start?").required(true);
    for (profile, members) in workspace.profiles() {
        let members: Vec<&str> = members.iter().map(ServiceName::as_str).collect();
        let token = format!("@{profile}");
        prompt = prompt.item(token.clone(), token, members.join(", "));
    }
    for service in workspace.services() {
        prompt = prompt.item(service.name.to_string(), &service.name, hint(service));
    }
    let targets = prompt.interact().map_err(|_| anyhow!("cancelled"))?;
    cliclack::outro(format!("starting {}", targets.join(" ")))?;
    Ok(targets)
}

fn hint(service: &Service) -> String {
    let ports: Vec<String> = service
        .ports
        .iter()
        .map(|port| format!(":{port}"))
        .collect();
    if ports.is_empty() {
        service.runtime.to_string()
    } else {
        format!("{} · {}", service.runtime, ports.join(" "))
    }
}
