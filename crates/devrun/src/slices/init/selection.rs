use std::collections::{BTreeMap, BTreeSet};

use devrun_domain::{AliasName, Candidate, Confidence, Port, RepoWorkspace, Service, ServiceName};

const WORD_SEPARATORS: [char; 3] = ['-', '_', '.'];

pub fn accepted_by_default(candidate: &Candidate) -> bool {
    matches!(candidate.confidence, Confidence::High | Confidence::Medium)
}

pub fn is_new(workspace: &RepoWorkspace, candidate: &Candidate) -> bool {
    workspace.service(&candidate.service.name).is_none()
}

pub fn adopt(workspace: &mut RepoWorkspace, services: Vec<Service>) -> Vec<String> {
    let incoming: BTreeSet<ServiceName> = services
        .iter()
        .map(|service| service.name.clone())
        .collect();
    let mut problems = Vec::new();
    let mut pending: Vec<Service> = services
        .into_iter()
        .map(|service| without_missing_needs(workspace, &incoming, service, &mut problems))
        .collect();
    while !pending.is_empty() {
        let (ready, waiting): (Vec<Service>, Vec<Service>) = pending
            .into_iter()
            .partition(|service| needs_are_present(workspace, service));
        let (batch, rest) = if ready.is_empty() {
            (waiting, Vec::new())
        } else {
            (ready, waiting)
        };
        for service in batch {
            if let Err(error) = workspace.upsert_service(service) {
                problems.push(error.to_string());
            }
        }
        pending = rest;
    }
    problems
}

fn needs_are_present(workspace: &RepoWorkspace, service: &Service) -> bool {
    service
        .needs
        .iter()
        .all(|need| workspace.service(need).is_some())
}

fn without_missing_needs(
    workspace: &RepoWorkspace,
    incoming: &BTreeSet<ServiceName>,
    mut service: Service,
    problems: &mut Vec<String>,
) -> Service {
    let (kept, dropped): (Vec<ServiceName>, Vec<ServiceName>) = service
        .needs
        .into_iter()
        .partition(|need| incoming.contains(need) || workspace.service(need).is_some());
    for need in dropped {
        problems.push(format!(
            "{} no longer needs {need}: it was not selected",
            service.name
        ));
    }
    service.needs = kept;
    service
}

pub fn port_claims<'a>(
    services: impl IntoIterator<Item = &'a Service>,
) -> Vec<(Port, Vec<ServiceName>)> {
    let mut claims: BTreeMap<Port, BTreeSet<ServiceName>> = BTreeMap::new();
    for service in services {
        for port in &service.ports {
            claims
                .entry(*port)
                .or_default()
                .insert(service.name.clone());
        }
    }
    claims
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(port, names)| (port, names.into_iter().collect()))
        .collect()
}

pub fn suggest_aliases(
    workspace: &RepoWorkspace,
    added: &[ServiceName],
) -> Vec<(AliasName, ServiceName)> {
    let aliased: BTreeSet<&ServiceName> = workspace.aliases().values().collect();
    let mut by_initials: BTreeMap<String, Vec<ServiceName>> = BTreeMap::new();
    for name in added.iter().filter(|name| !aliased.contains(name)) {
        if let Some(initials) = initials(name) {
            by_initials.entry(initials).or_default().push(name.clone());
        }
    }
    by_initials
        .into_iter()
        .filter_map(|(initials, mut names)| {
            let alias = AliasName::parse(&initials).ok()?;
            let free = names.len() == 1
                && !workspace.aliases().contains_key(&alias)
                && !workspace
                    .services()
                    .any(|service| service.name.as_str() == initials);
            free.then(|| (alias, names.remove(0)))
        })
        .collect()
}

fn initials(name: &ServiceName) -> Option<String> {
    let words: Vec<&str> = name
        .as_str()
        .split(WORD_SEPARATORS)
        .filter(|word| !word.is_empty())
        .collect();
    (words.len() > 1).then(|| {
        words
            .iter()
            .filter_map(|word| word.chars().next())
            .collect()
    })
}

pub fn next_step(workspace: &RepoWorkspace) -> Option<String> {
    let first_profile = workspace
        .profiles()
        .keys()
        .next()
        .map(|profile| format!("@{profile}"));
    let first_service = || {
        workspace
            .services()
            .next()
            .map(|service| service.name.to_string())
    };
    first_profile
        .or_else(first_service)
        .map(|target| format!("devrun up {target}"))
}

pub fn joined(names: &[ServiceName]) -> String {
    names
        .iter()
        .map(ServiceName::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn counted(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}
