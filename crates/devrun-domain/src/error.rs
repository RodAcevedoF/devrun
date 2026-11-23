use std::fmt::Display;
use std::path::PathBuf;

use thiserror::Error;

use crate::{AliasName, Port, ProfileName, ServiceName};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DomainError {
    #[error(
        "invalid {kind} name `{value}`: use 1-64 lowercase letters, digits, `-`, `_` or `.`, starting with a letter or digit"
    )]
    InvalidName { kind: &'static str, value: String },
    #[error("invalid port `0`: ports must be between 1 and 65535")]
    InvalidPort,
    #[error("invalid path `{value}`: {reason}")]
    InvalidPath { value: String, reason: &'static str },
    #[error("command must not be empty")]
    EmptyCommand,
    #[error("invalid environment variable name `{0}`")]
    InvalidEnvKey(String),
    #[error("unknown runtime `{0}`: expected node, python, compose or custom")]
    InvalidRuntime(String),
    #[error("repository root `{}` must be an absolute path", .0.display())]
    RootNotAbsolute(PathBuf),
    #[error("service `{0}` is defined more than once")]
    DuplicateService(ServiceName),
    #[error("service `{service}` needs unknown service `{need}`")]
    UnknownNeed {
        service: ServiceName,
        need: ServiceName,
    },
    #[error("dependency cycle: {}", join(.0, " -> "))]
    DependencyCycle(Vec<ServiceName>),
    #[error("alias `{0}` has the same name as a service")]
    AliasShadowsService(AliasName),
    #[error("alias `{alias}` points to unknown service `{target}`")]
    UnknownAliasTarget {
        alias: AliasName,
        target: ServiceName,
    },
    #[error("profile `@{0}` must contain at least one service")]
    EmptyProfile(ProfileName),
    #[error("profile `@{profile}` includes unknown service `{member}`")]
    UnknownProfileMember {
        profile: ProfileName,
        member: ServiceName,
    },
    #[error("service `{service}` is still used by {}", join(.references, ", "))]
    ServiceInUse {
        service: ServiceName,
        references: Vec<String>,
    },
    #[error("unknown service or alias `{name}`{}", hint(.suggestion.as_deref()))]
    UnknownTarget {
        name: String,
        suggestion: Option<String>,
    },
    #[error("unknown alias `{0}`")]
    UnknownAlias(String),
    #[error("unknown profile `@{name}`{}", hint(.suggestion.as_deref()))]
    UnknownProfile {
        name: String,
        suggestion: Option<String>,
    },
    #[error("invalid target `{0}`: expected `name`, `@profile`, `+name` or `-name`")]
    InvalidTarget(String),
    #[error("nothing selected: name at least one service, alias or @profile")]
    EmptySelection,
    #[error("port {port} is used by both `{first}` and `{second}`")]
    PortConflict {
        port: Port,
        first: ServiceName,
        second: ServiceName,
    },
}

fn join<T: Display>(items: &[T], separator: &str) -> String {
    items
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(separator)
}

fn hint(suggestion: Option<&str>) -> String {
    suggestion
        .map(|name| format!(" (did you mean `{name}`?)"))
        .unwrap_or_default()
}
