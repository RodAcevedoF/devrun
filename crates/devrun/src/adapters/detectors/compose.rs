use std::path::{Path, PathBuf};

use devrun_domain::{Candidate, Confidence, Port, RelativePath, Runtime, Service};
use serde_yaml_ng::Value;

use super::ports::parse_port;
use super::walk::directories;
use super::{Detector, Discovery};

const COMPOSE_FILES: [&str; 4] = [
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yml",
    "compose.yaml",
];

pub struct ComposeFiles;

struct ComposeFile<'a> {
    root: &'a Path,
    location: PathBuf,
    file: &'static str,
    dir: RelativePath,
}

impl Detector for ComposeFiles {
    fn detect(&self, root: &Path) -> anyhow::Result<Discovery> {
        let mut discovery = Discovery::default();
        for dir in directories(root) {
            for file in COMPOSE_FILES {
                let location = dir.join(file);
                if location.is_file()
                    && let Some(dir) = discovery.relative(root, &dir)
                {
                    let compose = ComposeFile {
                        root,
                        location,
                        file,
                        dir,
                    };
                    compose.detect(&mut discovery);
                }
            }
        }
        Ok(discovery)
    }
}

impl ComposeFile<'_> {
    fn detect(&self, discovery: &mut Discovery) {
        let Some(text) = discovery.read(self.root, &self.location) else {
            return;
        };
        let document: Value = match serde_yaml_ng::from_str(&text) {
            Ok(document) => document,
            Err(error) => return discovery.skip(self.root, &self.location, error),
        };
        let Some(services) = document.get("services").and_then(Value::as_mapping) else {
            return;
        };
        for (key, definition) in services {
            if let Some(raw) = key.as_str()
                && let Some(candidate) = self.candidate(raw, definition, discovery)
            {
                discovery.candidates.push(candidate);
            }
        }
    }

    fn candidate(
        &self,
        raw: &str,
        definition: &Value,
        discovery: &mut Discovery,
    ) -> Option<Candidate> {
        let name = discovery.service_name(self.root, &self.location, raw)?;
        let operands: &[&str] = if raw.starts_with('-') {
            &["--", raw]
        } else {
            &[raw]
        };
        let command = discovery.command(
            self.root,
            &self.location,
            ["docker", "compose", "-f", self.file, "up"]
                .iter()
                .chain(operands),
        )?;
        let ports: Vec<Port> = definition
            .get("ports")
            .and_then(Value::as_sequence)
            .into_iter()
            .flatten()
            .filter_map(host_port)
            .collect();
        let service =
            Service::new(name, Runtime::Compose, self.dir.clone(), command).with_ports(ports);
        Some(Candidate::new(
            service,
            Confidence::High,
            format!("{} service \"{raw}\"", self.file),
        ))
    }
}

fn host_port(entry: &Value) -> Option<Port> {
    match entry {
        Value::String(short) => {
            let mapping = short.split('/').next()?;
            let parts: Vec<&str> = mapping.split(':').collect();
            let host = parts.len().checked_sub(2).map(|index| parts[index])?;
            parse_port(host)
        }
        Value::Mapping(_) => match entry.get("published")? {
            Value::Number(number) => number
                .as_u64()
                .and_then(|value| u16::try_from(value).ok())
                .and_then(|value| Port::new(value).ok()),
            Value::String(text) => parse_port(text),
            _ => None,
        },
        _ => None,
    }
}
