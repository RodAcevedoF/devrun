use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::{Port, RepoWorkspace, Runtime, Service, ServiceName};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

impl fmt::Display for Confidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub service: Service,
    pub confidence: Confidence,
    pub evidence: String,
}

impl Candidate {
    pub fn new(service: Service, confidence: Confidence, evidence: impl Into<String>) -> Self {
        Self {
            service,
            confidence,
            evidence: evidence.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub current: Service,
    pub candidate: Candidate,
}

impl Change {
    pub fn runtime_changed(&self) -> bool {
        self.current.runtime != self.candidate.service.runtime
    }

    pub fn path_changed(&self) -> bool {
        self.current.path != self.candidate.service.path
    }

    pub fn command_changed(&self) -> bool {
        self.current.command != self.candidate.service.command
    }

    pub fn ports_changed(&self) -> bool {
        discovered_set_differs(&self.current.ports, &self.candidate.service.ports)
    }

    pub fn needs_changed(&self) -> bool {
        discovered_set_differs(&self.current.needs, &self.candidate.service.needs)
    }

    pub fn prepare_changed(&self) -> bool {
        let discovered = &self.candidate.service.prepare;
        !discovered.is_empty() && self.current.prepare != *discovered
    }

    pub fn watch_changed(&self) -> bool {
        discovered_set_differs(&self.current.watch, &self.candidate.service.watch)
    }

    fn any_changed(&self) -> bool {
        self.runtime_changed()
            || self.path_changed()
            || self.command_changed()
            || self.ports_changed()
            || self.needs_changed()
            || self.prepare_changed()
            || self.watch_changed()
    }
}

fn discovered_set_differs<T: Ord>(current: &[T], discovered: &[T]) -> bool {
    !discovered.is_empty() && as_set(current) != as_set(discovered)
}

fn as_set<T: Ord>(values: &[T]) -> BTreeSet<&T> {
    values.iter().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DiscoveryDiff {
    pub added: Vec<Candidate>,
    pub changed: Vec<Change>,
    pub missing: Vec<ServiceName>,
}

impl DiscoveryDiff {
    pub fn between(workspace: &RepoWorkspace, candidates: &[Candidate]) -> Self {
        let mut sorted: Vec<&Candidate> = candidates.iter().collect();
        sorted.sort_by(|left, right| left.service.name.cmp(&right.service.name));
        let mut diff = Self::default();
        for candidate in sorted {
            match workspace.service(&candidate.service.name) {
                None => diff.added.push(candidate.clone()),
                Some(current) => {
                    let change = Change {
                        current: current.clone(),
                        candidate: candidate.clone(),
                    };
                    if change.any_changed() {
                        diff.changed.push(change);
                    }
                }
            }
        }
        let discovered: BTreeSet<&ServiceName> = candidates
            .iter()
            .map(|candidate| &candidate.service.name)
            .collect();
        diff.missing = workspace
            .services()
            .filter(|service| service.runtime != Runtime::Custom)
            .filter(|service| !discovered.contains(&service.name))
            .map(|service| service.name.clone())
            .collect();
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.missing.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPort {
    pub port: Port,
    pub services: Vec<ServiceName>,
}

pub fn shared_ports(candidates: &[Candidate]) -> Vec<SharedPort> {
    let mut claims: BTreeMap<Port, BTreeSet<ServiceName>> = BTreeMap::new();
    for candidate in candidates {
        for port in &candidate.service.ports {
            claims
                .entry(*port)
                .or_default()
                .insert(candidate.service.name.clone());
        }
    }
    claims
        .into_iter()
        .filter(|(_, services)| services.len() > 1)
        .map(|(port, services)| SharedPort {
            port,
            services: services.into_iter().collect(),
        })
        .collect()
}
