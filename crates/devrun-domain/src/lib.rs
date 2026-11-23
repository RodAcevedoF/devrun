mod discovery;
mod error;
mod identifier;
mod plan;
mod ports;
mod service;
mod suggest;
mod target;
mod workspace;

pub use discovery::{Candidate, Change, Confidence, DiscoveryDiff, SharedPort, shared_ports};
pub use error::DomainError;
pub use identifier::{AliasName, ProfileName, ServiceName};
pub use plan::{PlanReason, PlannedService, RunPlan, SkippedNeed};
pub use ports::WorkspaceRepository;
pub use service::{CommandLine, EnvKey, Port, RelativePath, Runtime, Service};
pub use target::{TargetExpr, TargetOp, TargetRef};
pub use workspace::RepoWorkspace;
