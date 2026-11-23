use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::anyhow;
use devrun_domain::{
    Candidate, DomainError, Port, ProfileName, RepoWorkspace, Runtime, Service, ServiceName,
    WorkspaceRepository,
};

use super::prompter::{CANCELLED, Choice, Prompter};
use super::selection::{
    accepted_by_default, adopt, counted, is_new, joined, next_step, port_claims, suggest_aliases,
};
use crate::adapters::detectors::Discovery;

const SERVICES_PROMPT: &str = "Services to keep";
const ALIASES_PROMPT: &str = "Aliases to create";

pub struct Repo {
    pub root: PathBuf,
    pub label: String,
    pub config_path: PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Saved,
    Cancelled,
}

enum Stop {
    Cancelled,
    Failed(anyhow::Error),
}

impl From<io::Error> for Stop {
    fn from(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::Interrupted {
            Self::Cancelled
        } else {
            Self::Failed(error.into())
        }
    }
}

impl From<anyhow::Error> for Stop {
    fn from(error: anyhow::Error) -> Self {
        Self::Failed(error)
    }
}

impl From<DomainError> for Stop {
    fn from(error: DomainError) -> Self {
        Self::Failed(error.into())
    }
}

struct Offer {
    service: Service,
    saved: bool,
    checked: bool,
}

pub fn run<P, R>(
    prompter: &mut P,
    store: &R,
    repo: &Repo,
    discover: impl FnOnce(&Path) -> anyhow::Result<Discovery>,
) -> anyhow::Result<Outcome>
where
    P: Prompter,
    R: WorkspaceRepository<Error = anyhow::Error>,
{
    match converse(prompter, store, repo, discover) {
        Ok(()) => Ok(Outcome::Saved),
        Err(Stop::Cancelled) => {
            prompter.cancelled(CANCELLED)?;
            Ok(Outcome::Cancelled)
        }
        Err(Stop::Failed(error)) => Err(error),
    }
}

fn converse<P, R>(
    prompter: &mut P,
    store: &R,
    repo: &Repo,
    discover: impl FnOnce(&Path) -> anyhow::Result<Discovery>,
) -> Result<(), Stop>
where
    P: Prompter,
    R: WorkspaceRepository<Error = anyhow::Error>,
{
    prompter.intro(&format!("devrun init · {}", repo.label))?;
    let mut workspace = match store.load(&repo.root)? {
        Some(workspace) => workspace,
        None => RepoWorkspace::new(repo.root.clone())?,
    };
    let discovery = discover_with_spinner(prompter, &repo.root, discover)?;
    prompter.note(
        &format!(
            "Discovered {}",
            counted(discovery.candidates.len(), "service", "services")
        ),
        &discovery_summary(&discovery),
    )?;
    let mut incoming = choose_services(prompter, &mut workspace, &discovery.candidates)?;
    reassign_ports(prompter, &mut workspace, &mut incoming)?;
    let added = add_services(prompter, &mut workspace, incoming)?;
    choose_aliases(prompter, &mut workspace, &added)?;
    create_profiles(prompter, &mut workspace)?;
    prompter.note("Summary", &workspace_summary(&workspace))?;
    if !prompter.confirm(&format!("Save to {}?", repo.config_path.display()), true)? {
        return Err(Stop::Cancelled);
    }
    store.save(&workspace)?;
    prompter.outro(
        &next_step(&workspace).map_or_else(|| "Saved".to_owned(), |step| format!("Next: {step}")),
    )?;
    Ok(())
}

fn discover_with_spinner(
    prompter: &mut impl Prompter,
    root: &Path,
    discover: impl FnOnce(&Path) -> anyhow::Result<Discovery>,
) -> Result<Discovery, Stop> {
    prompter.start_spinner("Discovering services");
    let discovery = discover(root);
    prompter.stop_spinner(if discovery.is_ok() {
        "Discovery finished"
    } else {
        "Discovery failed"
    });
    Ok(discovery?)
}

fn discovery_summary(discovery: &Discovery) -> String {
    let mut per_runtime: BTreeMap<Runtime, usize> = BTreeMap::new();
    for candidate in &discovery.candidates {
        *per_runtime.entry(candidate.service.runtime).or_default() += 1;
    }
    let counts = per_runtime
        .into_iter()
        .map(|(runtime, count)| format!("{:<8} {count}", runtime.as_str()));
    let warnings = discovery
        .warnings
        .iter()
        .map(|warning| format!("warning: {warning}"));
    let lines: Vec<String> = counts.chain(warnings).collect();
    if lines.is_empty() {
        "nothing found".to_owned()
    } else {
        lines.join("\n")
    }
}

fn offers(workspace: &RepoWorkspace, candidates: &[Candidate]) -> Vec<Offer> {
    let saved = workspace.services().map(|service| Offer {
        service: service.clone(),
        saved: true,
        checked: true,
    });
    let discovered = candidates
        .iter()
        .filter(|candidate| is_new(workspace, candidate))
        .map(|candidate| Offer {
            service: candidate.service.clone(),
            saved: false,
            checked: accepted_by_default(candidate),
        });
    let mut offers: Vec<Offer> = saved.chain(discovered).collect();
    offers.sort_by(|left, right| left.service.name.cmp(&right.service.name));
    offers
}

fn offer_choice(offer: &Offer, rerun: bool) -> Choice {
    let service = &offer.service;
    let details = format!(
        "{} · {} · {}",
        service.runtime, service.path, service.command
    );
    Choice {
        label: service.name.to_string(),
        hint: if rerun && !offer.saved {
            format!("new · {details}")
        } else {
            details
        },
        checked: offer.checked,
    }
}

fn choose_services(
    prompter: &mut impl Prompter,
    workspace: &mut RepoWorkspace,
    candidates: &[Candidate],
) -> Result<Vec<Service>, Stop> {
    let offers = offers(workspace, candidates);
    if offers.is_empty() {
        prompter.warning("no services found; add them later with `devrun init`")?;
        return Ok(Vec::new());
    }
    let rerun = workspace.services().next().is_some();
    let choices: Vec<Choice> = offers
        .iter()
        .map(|offer| offer_choice(offer, rerun))
        .collect();
    let picked: BTreeSet<usize> = prompter
        .multiselect(SERVICES_PROMPT, &choices, false)?
        .into_iter()
        .collect();
    let mut incoming = Vec::new();
    let mut dropped = Vec::new();
    for (index, offer) in offers.into_iter().enumerate() {
        match (offer.saved, picked.contains(&index)) {
            (false, true) => incoming.push(offer.service),
            (true, false) => dropped.push(offer.service.name),
            _ => {}
        }
    }
    remove_services(prompter, workspace, dropped)?;
    Ok(incoming)
}

fn remove_services(
    prompter: &mut impl Prompter,
    workspace: &mut RepoWorkspace,
    mut pending: Vec<ServiceName>,
) -> io::Result<()> {
    loop {
        let before = pending.len();
        pending.retain(|name| workspace.remove_service(name).is_err());
        if pending.is_empty() || pending.len() == before {
            break;
        }
    }
    for name in pending {
        if let Err(error) = workspace.remove_service(&name) {
            prompter.error(&format!("kept {name}: {error}"))?;
        }
    }
    Ok(())
}

fn reassign_ports(
    prompter: &mut impl Prompter,
    workspace: &mut RepoWorkspace,
    incoming: &mut [Service],
) -> Result<(), Stop> {
    let claims = port_claims(workspace.services().chain(incoming.iter()));
    for (port, claimants) in claims {
        prompter.warning(&format!("port {port} is claimed by {}", joined(&claimants)))?;
        for name in claimants {
            let answer =
                prompter.input(&format!("Port for {name}"), &port.to_string(), check_port)?;
            let chosen = parse_port(&answer).map_err(|error| Stop::Failed(anyhow!(error)))?;
            if chosen != port {
                move_port(workspace, incoming, &name, port, chosen)?;
            }
        }
    }
    Ok(())
}

fn parse_port(raw: &str) -> Result<Port, String> {
    let trimmed = raw.trim();
    let value = trimmed
        .parse::<u16>()
        .map_err(|_| format!("`{trimmed}` is not a port number"))?;
    Port::new(value).map_err(|error| error.to_string())
}

fn check_port(raw: &str) -> Result<(), String> {
    parse_port(raw).map(|_| ())
}

fn move_port(
    workspace: &mut RepoWorkspace,
    incoming: &mut [Service],
    name: &ServiceName,
    from: Port,
    to: Port,
) -> Result<(), DomainError> {
    let replace = |service: &mut Service| {
        for port in &mut service.ports {
            if *port == from {
                *port = to;
            }
        }
    };
    if let Some(service) = incoming.iter_mut().find(|service| &service.name == name) {
        replace(service);
        return Ok(());
    }
    match workspace.service(name).cloned() {
        Some(mut service) => {
            replace(&mut service);
            workspace.upsert_service(service)
        }
        None => Ok(()),
    }
}

fn add_services(
    prompter: &mut impl Prompter,
    workspace: &mut RepoWorkspace,
    incoming: Vec<Service>,
) -> io::Result<Vec<ServiceName>> {
    let names: Vec<ServiceName> = incoming
        .iter()
        .map(|service| service.name.clone())
        .collect();
    for problem in adopt(workspace, incoming) {
        prompter.warning(&problem)?;
    }
    Ok(names
        .into_iter()
        .filter(|name| workspace.service(name).is_some())
        .collect())
}

fn choose_aliases(
    prompter: &mut impl Prompter,
    workspace: &mut RepoWorkspace,
    added: &[ServiceName],
) -> io::Result<()> {
    let suggestions = suggest_aliases(workspace, added);
    if suggestions.is_empty() {
        return Ok(());
    }
    let choices: Vec<Choice> = suggestions
        .iter()
        .map(|(alias, target)| Choice {
            label: alias.to_string(),
            hint: format!("-> {target}"),
            checked: true,
        })
        .collect();
    let picked = prompter.multiselect(ALIASES_PROMPT, &choices, false)?;
    for index in picked {
        if let Some((alias, target)) = suggestions.get(index).cloned()
            && let Err(error) = workspace.set_alias(alias, target)
        {
            prompter.error(&error.to_string())?;
        }
    }
    Ok(())
}

fn parse_profile_name(raw: &str) -> Result<ProfileName, DomainError> {
    let trimmed = raw.trim();
    ProfileName::parse(trimmed.strip_prefix('@').unwrap_or(trimmed))
}

fn check_profile_name(raw: &str) -> Result<(), String> {
    parse_profile_name(raw)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn create_profiles(
    prompter: &mut impl Prompter,
    workspace: &mut RepoWorkspace,
) -> Result<(), Stop> {
    let services: Vec<ServiceName> = workspace
        .services()
        .map(|service| service.name.clone())
        .collect();
    if services.is_empty() {
        return Ok(());
    }
    let mut question = "Create a profile?";
    while prompter.confirm(question, false)? {
        let name = parse_profile_name(&prompter.input("Profile name", "", check_profile_name)?)?;
        let current = workspace.profiles().get(&name).cloned().unwrap_or_default();
        let choices: Vec<Choice> = services
            .iter()
            .map(|service| Choice {
                label: service.to_string(),
                hint: String::new(),
                checked: current.contains(service),
            })
            .collect();
        let members: Vec<ServiceName> = prompter
            .multiselect(&format!("Services for @{name}"), &choices, true)?
            .into_iter()
            .filter_map(|index| services.get(index).cloned())
            .collect();
        if let Err(error) = workspace.set_profile(name, members) {
            prompter.error(&error.to_string())?;
        }
        question = "Create another profile?";
    }
    Ok(())
}

fn workspace_summary(workspace: &RepoWorkspace) -> String {
    let services: Vec<ServiceName> = workspace
        .services()
        .map(|service| service.name.clone())
        .collect();
    let aliases: Vec<String> = workspace
        .aliases()
        .iter()
        .map(|(alias, target)| format!("{alias} -> {target}"))
        .collect();
    let profiles: Vec<String> = workspace
        .profiles()
        .iter()
        .map(|(profile, members)| format!("@{profile} ({})", joined(members)))
        .collect();
    [
        ("Services", joined(&services)),
        ("Aliases", aliases.join(", ")),
        ("Profiles", profiles.join(", ")),
    ]
    .into_iter()
    .map(|(title, body)| {
        let body = if body.is_empty() {
            "(none)".to_owned()
        } else {
            body
        };
        format!("{title:<9} {body}")
    })
    .collect::<Vec<_>>()
    .join("\n")
}
