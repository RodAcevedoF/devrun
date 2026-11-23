mod compose;
mod names;
mod node;
mod ports;
mod python;
mod walk;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs;
use std::path::Path;

use devrun_domain::{Candidate, RelativePath, Runtime, ServiceName};

use compose::ComposeFiles;
use node::NodeWorkspaces;
use python::PythonProjects;

pub trait Detector {
    fn detect(&self, root: &Path) -> anyhow::Result<Discovery>;
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub candidates: Vec<Candidate>,
    pub warnings: Vec<String>,
}

impl Discovery {
    fn skip(&mut self, root: &Path, manifest: &Path, reason: impl Display) {
        let shown = manifest.strip_prefix(root).unwrap_or(manifest);
        let reason = reason.to_string();
        let first_line = reason.lines().next().unwrap_or_default();
        self.warnings
            .push(format!("skipped {}: {first_line}", shown.display()));
    }

    fn read(&mut self, root: &Path, manifest: &Path) -> Option<String> {
        fs::read_to_string(manifest)
            .map_err(|error| self.skip(root, manifest, error))
            .ok()
    }

    fn relative(&mut self, root: &Path, dir: &Path) -> Option<RelativePath> {
        let relative = dir.strip_prefix(root).unwrap_or(dir);
        RelativePath::parse(&relative.to_string_lossy())
            .map_err(|error| self.skip(root, dir, error))
            .ok()
    }

    fn service_name(&mut self, root: &Path, dir: &Path, raw: &str) -> Option<ServiceName> {
        ServiceName::slug(raw)
            .map_err(|error| self.skip(root, dir, error))
            .ok()
    }

    fn absorb(&mut self, other: Self) {
        self.candidates.extend(other.candidates);
        self.warnings.extend(other.warnings);
    }
}

pub fn detect_all(root: &Path) -> anyhow::Result<Discovery> {
    let detectors: [&dyn Detector; 3] = [&NodeWorkspaces, &PythonProjects, &ComposeFiles];
    let mut discovery = Discovery::default();
    for detector in detectors {
        discovery.absorb(detector.detect(root)?);
    }
    names::disambiguate(&mut discovery.candidates);
    link_compose_needs(&mut discovery.candidates);
    discovery
        .candidates
        .sort_by(|left, right| left.service.name.cmp(&right.service.name));
    Ok(discovery)
}

fn link_compose_needs(candidates: &mut [Candidate]) {
    let mut compose_by_dir: BTreeMap<RelativePath, Vec<ServiceName>> = BTreeMap::new();
    for candidate in candidates.iter() {
        if candidate.service.runtime == Runtime::Compose {
            compose_by_dir
                .entry(candidate.service.path.clone())
                .or_default()
                .push(candidate.service.name.clone());
        }
    }
    for candidate in candidates
        .iter_mut()
        .filter(|candidate| matches!(candidate.service.runtime, Runtime::Node | Runtime::Python))
    {
        if let Some(needs) = compose_by_dir.get(&candidate.service.path) {
            let mut needs = needs.clone();
            needs.sort();
            let listed: Vec<&str> = needs.iter().map(ServiceName::as_str).collect();
            candidate.evidence = format!(
                "{}; needs compose services {}",
                candidate.evidence,
                listed.join(", ")
            );
            candidate.service.needs = needs;
        }
    }
}
