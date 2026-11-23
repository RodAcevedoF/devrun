use std::collections::{BTreeMap, BTreeSet};
use std::path::Component;

use devrun_domain::{Candidate, RelativePath, Runtime, ServiceName};

type Rename = fn(&Candidate, usize) -> String;

const RENAMES: [Rename; 3] = [
    with_directory_prefix,
    with_runtime_suffix,
    with_position_suffix,
];

pub fn disambiguate(candidates: &mut [Candidate]) {
    candidates.sort_by(|left, right| identity(left).cmp(&identity(right)));
    for rename in RENAMES {
        let colliding = colliding_names(candidates);
        if colliding.is_empty() {
            return;
        }
        let mut positions: BTreeMap<ServiceName, usize> = BTreeMap::new();
        for candidate in candidates
            .iter_mut()
            .filter(|candidate| colliding.contains(&candidate.service.name))
        {
            let position = positions.entry(candidate.service.name.clone()).or_default();
            if let Ok(name) = ServiceName::slug(&rename(candidate, *position)) {
                candidate.service.name = name;
            }
            *position += 1;
        }
    }
}

fn identity(candidate: &Candidate) -> (&ServiceName, &RelativePath, Runtime, &str) {
    (
        &candidate.service.name,
        &candidate.service.path,
        candidate.service.runtime,
        candidate.service.command.as_str(),
    )
}

fn colliding_names(candidates: &[Candidate]) -> BTreeSet<ServiceName> {
    let mut seen = BTreeSet::new();
    candidates
        .iter()
        .map(|candidate| &candidate.service.name)
        .filter(|name| !seen.insert(*name))
        .cloned()
        .collect()
}

fn with_directory_prefix(candidate: &Candidate, _: usize) -> String {
    let name = candidate.service.name.as_str();
    let prefix = candidate
        .service
        .path
        .as_path()
        .components()
        .rev()
        .filter_map(|component| match component {
            Component::Normal(part) => ServiceName::slug(&part.to_string_lossy()).ok(),
            _ => None,
        })
        .find(|slug| slug.as_str() != name);
    prefix.map_or_else(|| name.to_owned(), |prefix| format!("{prefix}-{name}"))
}

fn with_runtime_suffix(candidate: &Candidate, _: usize) -> String {
    format!("{}-{}", candidate.service.name, candidate.service.runtime)
}

fn with_position_suffix(candidate: &Candidate, position: usize) -> String {
    if position == 0 {
        candidate.service.name.to_string()
    } else {
        format!("{}-{}", candidate.service.name, position + 1)
    }
}
