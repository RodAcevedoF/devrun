use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use crate::{DomainError, ServiceName};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Port(u16);

impl Port {
    pub fn new(value: u16) -> Result<Self, DomainError> {
        if value == 0 {
            Err(DomainError::InvalidPort)
        } else {
            Ok(Self(value))
        }
    }

    pub fn get(self) -> u16 {
        self.0
    }
}

impl fmt::Display for Port {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelativePath(PathBuf);

impl RelativePath {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let invalid = |reason| DomainError::InvalidPath {
            value: raw.to_owned(),
            reason,
        };
        let mut normalized = PathBuf::new();
        for component in Path::new(raw).components() {
            match component {
                Component::Normal(part) => normalized.push(part),
                Component::CurDir => {}
                Component::ParentDir => return Err(invalid("must not leave the repository")),
                Component::RootDir | Component::Prefix(_) => {
                    return Err(invalid("must be relative to the repository root"));
                }
            }
        }
        Ok(Self(normalized))
    }

    pub fn repository_root() -> Self {
        Self(PathBuf::new())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn within(&self, root: &Path) -> PathBuf {
        root.join(&self.0)
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.as_os_str().is_empty() {
            f.write_str(".")
        } else {
            write!(f, "{}", self.0.display())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandLine(String);

impl CommandLine {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            Err(DomainError::EmptyCommand)
        } else {
            Ok(Self(trimmed.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommandLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EnvKey(String);

impl EnvKey {
    pub fn parse(raw: &str) -> Result<Self, DomainError> {
        let mut chars = raw.chars();
        let valid = chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
        if valid {
            Ok(Self(raw.to_owned()))
        } else {
            Err(DomainError::InvalidEnvKey(raw.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EnvKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Runtime {
    Node,
    Python,
    Compose,
    Custom,
}

impl Runtime {
    pub const ALL: [Self; 4] = [Self::Node, Self::Python, Self::Compose, Self::Custom];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Python => "python",
            Self::Compose => "compose",
            Self::Custom => "custom",
        }
    }
}

impl fmt::Display for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Runtime {
    type Err = DomainError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|runtime| runtime.as_str() == raw)
            .ok_or_else(|| DomainError::InvalidRuntime(raw.to_owned()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    pub name: ServiceName,
    pub runtime: Runtime,
    pub path: RelativePath,
    pub command: CommandLine,
    pub env: BTreeMap<EnvKey, String>,
    pub ports: Vec<Port>,
    pub needs: Vec<ServiceName>,
    pub prepare: Vec<CommandLine>,
    pub watch: Vec<RelativePath>,
}

impl Service {
    pub fn new(
        name: ServiceName,
        runtime: Runtime,
        path: RelativePath,
        command: CommandLine,
    ) -> Self {
        Self {
            name,
            runtime,
            path,
            command,
            env: BTreeMap::new(),
            ports: Vec::new(),
            needs: Vec::new(),
            prepare: Vec::new(),
            watch: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_ports(mut self, ports: impl IntoIterator<Item = Port>) -> Self {
        self.ports = ports.into_iter().collect();
        self
    }

    #[must_use]
    pub fn with_needs(mut self, needs: impl IntoIterator<Item = ServiceName>) -> Self {
        self.needs = needs.into_iter().collect();
        self
    }

    #[must_use]
    pub fn with_prepare(mut self, prepare: impl IntoIterator<Item = CommandLine>) -> Self {
        self.prepare = prepare.into_iter().collect();
        self
    }

    #[must_use]
    pub fn with_watch(mut self, dirs: impl IntoIterator<Item = RelativePath>) -> Self {
        self.watch = dirs
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self
    }

    #[must_use]
    pub fn with_env(mut self, env: impl IntoIterator<Item = (EnvKey, String)>) -> Self {
        self.env = env.into_iter().collect();
        self
    }
}
