use std::collections::BTreeMap;
use std::path::PathBuf;

use devrun_domain::{
    AliasName, CommandLine, DomainError, EnvKey, Port, ProfileName, RelativePath, RepoWorkspace,
    Service, ServiceName,
};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: i64 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceFile {
    pub schema_version: i64,
    pub root: PathBuf,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub services: BTreeMap<String, ServiceEntry>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub profiles: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceEntry {
    pub runtime: String,
    pub path: String,
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<u16>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prepare: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub watch: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

impl WorkspaceFile {
    pub fn from_domain(workspace: &RepoWorkspace) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            root: workspace.root().to_path_buf(),
            services: workspace
                .services()
                .map(|service| (service.name.to_string(), ServiceEntry::from_domain(service)))
                .collect(),
            aliases: workspace
                .aliases()
                .iter()
                .map(|(alias, target)| (alias.to_string(), target.to_string()))
                .collect(),
            profiles: workspace
                .profiles()
                .iter()
                .map(|(profile, members)| (profile.to_string(), strings(members)))
                .collect(),
        }
    }

    pub fn into_domain(self) -> Result<RepoWorkspace, DomainError> {
        let services = self
            .services
            .into_iter()
            .map(|(name, entry)| entry.into_domain(&name))
            .collect::<Result<Vec<_>, _>>()?;
        let aliases = self
            .aliases
            .into_iter()
            .map(|(alias, target)| Ok((AliasName::parse(&alias)?, ServiceName::parse(&target)?)))
            .collect::<Result<Vec<_>, DomainError>>()?;
        let profiles = self
            .profiles
            .into_iter()
            .map(|(profile, members)| {
                Ok((
                    ProfileName::parse(&profile)?,
                    parse_all(&members, ServiceName::parse)?,
                ))
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        RepoWorkspace::from_parts(self.root, services, aliases, profiles)
    }
}

impl ServiceEntry {
    fn from_domain(service: &Service) -> Self {
        Self {
            runtime: service.runtime.to_string(),
            path: service.path.to_string(),
            command: service.command.to_string(),
            ports: service.ports.iter().map(|port| port.get()).collect(),
            needs: strings(&service.needs),
            prepare: strings(&service.prepare),
            watch: strings(&service.watch),
            env: service
                .env
                .iter()
                .map(|(key, value)| (key.to_string(), value.clone()))
                .collect(),
        }
    }

    fn into_domain(self, name: &str) -> Result<Service, DomainError> {
        let ports = self
            .ports
            .into_iter()
            .map(Port::new)
            .collect::<Result<Vec<_>, _>>()?;
        let env = self
            .env
            .into_iter()
            .map(|(key, value)| Ok((EnvKey::parse(&key)?, value)))
            .collect::<Result<Vec<_>, DomainError>>()?;
        Ok(Service::new(
            ServiceName::parse(name)?,
            self.runtime.parse()?,
            RelativePath::parse(&self.path)?,
            CommandLine::parse(&self.command)?,
        )
        .with_ports(ports)
        .with_needs(parse_all(&self.needs, ServiceName::parse)?)
        .with_prepare(parse_all(&self.prepare, CommandLine::parse)?)
        .with_watch(parse_all(&self.watch, RelativePath::parse)?)
        .with_env(env))
    }
}

fn strings<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

fn parse_all<T>(
    raw: &[String],
    parse: impl Fn(&str) -> Result<T, DomainError>,
) -> Result<Vec<T>, DomainError> {
    raw.iter().map(|item| parse(item)).collect()
}
