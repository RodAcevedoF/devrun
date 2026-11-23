use std::collections::{BTreeMap, BTreeSet};

use crate::{DomainError, RepoWorkspace, Service, ServiceName, TargetExpr, TargetOp, TargetRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanReason {
    Requested,
    NeededBy(ServiceName),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedService {
    pub service: Service,
    pub reason: PlanReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedNeed {
    pub service: ServiceName,
    pub need: ServiceName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlan {
    services: Vec<PlannedService>,
    skipped_needs: Vec<SkippedNeed>,
}

impl RunPlan {
    pub fn resolve(workspace: &RepoWorkspace, expr: &TargetExpr) -> Result<Self, DomainError> {
        let selection = Selection::evaluate(workspace, expr)?;
        if selection.requested.is_empty() {
            return Err(DomainError::EmptySelection);
        }
        let mut builder = PlanBuilder {
            workspace,
            excluded: &selection.excluded,
            services: Vec::new(),
            skipped_needs: Vec::new(),
        };
        for name in &selection.requested {
            builder.place(name, &PlanReason::Requested);
        }
        let plan = Self {
            services: builder.services,
            skipped_needs: builder.skipped_needs,
        };
        plan.check_ports()?;
        Ok(plan)
    }

    pub fn services(&self) -> &[PlannedService] {
        &self.services
    }

    pub fn skipped_needs(&self) -> &[SkippedNeed] {
        &self.skipped_needs
    }

    fn check_ports(&self) -> Result<(), DomainError> {
        let mut owners = BTreeMap::new();
        for planned in &self.services {
            for port in &planned.service.ports {
                if let Some(first) = owners.insert(*port, &planned.service.name) {
                    return Err(DomainError::PortConflict {
                        port: *port,
                        first: first.clone(),
                        second: planned.service.name.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

struct Selection {
    requested: Vec<ServiceName>,
    excluded: BTreeSet<ServiceName>,
}

impl Selection {
    fn evaluate(workspace: &RepoWorkspace, expr: &TargetExpr) -> Result<Self, DomainError> {
        let mut selection = Self {
            requested: Vec::new(),
            excluded: BTreeSet::new(),
        };
        for op in expr.ops() {
            match op {
                TargetOp::Include(target) => {
                    for name in expand(workspace, target)? {
                        selection.excluded.remove(&name);
                        if !selection.requested.contains(&name) {
                            selection.requested.push(name);
                        }
                    }
                }
                TargetOp::Exclude(target) => {
                    for name in expand(workspace, target)? {
                        selection.requested.retain(|requested| *requested != name);
                        selection.excluded.insert(name);
                    }
                }
            }
        }
        Ok(selection)
    }
}

fn expand(workspace: &RepoWorkspace, target: &TargetRef) -> Result<Vec<ServiceName>, DomainError> {
    match target {
        TargetRef::Name(raw) => Ok(vec![workspace.resolve_name(raw)?.name.clone()]),
        TargetRef::Profile(raw) => Ok(workspace.resolve_profile(raw)?.to_vec()),
    }
}

struct PlanBuilder<'a> {
    workspace: &'a RepoWorkspace,
    excluded: &'a BTreeSet<ServiceName>,
    services: Vec<PlannedService>,
    skipped_needs: Vec<SkippedNeed>,
}

impl PlanBuilder<'_> {
    fn place(&mut self, name: &ServiceName, reason: &PlanReason) {
        if let Some(existing) = self
            .services
            .iter_mut()
            .find(|planned| planned.service.name == *name)
        {
            if *reason == PlanReason::Requested {
                existing.reason = PlanReason::Requested;
            }
            return;
        }
        let Some(service) = self.workspace.service(name) else {
            return;
        };
        for need in &service.needs {
            if self.excluded.contains(need) {
                self.skipped_needs.push(SkippedNeed {
                    service: name.clone(),
                    need: need.clone(),
                });
            } else {
                self.place(need, &PlanReason::NeededBy(name.clone()));
            }
        }
        self.services.push(PlannedService {
            service: service.clone(),
            reason: reason.clone(),
        });
    }
}
