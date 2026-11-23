use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::suggest::closest;
use crate::{AliasName, DomainError, ProfileName, Service, ServiceName};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoWorkspace {
    root: PathBuf,
    services: BTreeMap<ServiceName, Service>,
    aliases: BTreeMap<AliasName, ServiceName>,
    profiles: BTreeMap<ProfileName, Vec<ServiceName>>,
}

impl RepoWorkspace {
    pub fn new(root: PathBuf) -> Result<Self, DomainError> {
        if !root.is_absolute() {
            return Err(DomainError::RootNotAbsolute(root));
        }
        Ok(Self {
            root,
            services: BTreeMap::new(),
            aliases: BTreeMap::new(),
            profiles: BTreeMap::new(),
        })
    }

    pub fn from_parts(
        root: PathBuf,
        services: impl IntoIterator<Item = Service>,
        aliases: impl IntoIterator<Item = (AliasName, ServiceName)>,
        profiles: impl IntoIterator<Item = (ProfileName, Vec<ServiceName>)>,
    ) -> Result<Self, DomainError> {
        let mut workspace = Self::new(root)?;
        for service in services {
            let name = service.name.clone();
            if workspace.services.insert(name.clone(), service).is_some() {
                return Err(DomainError::DuplicateService(name));
            }
        }
        workspace.aliases = aliases.into_iter().collect();
        workspace.profiles = profiles
            .into_iter()
            .map(|(name, members)| (name, unique(members)))
            .collect();
        workspace.validate()?;
        Ok(workspace)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn services(&self) -> impl Iterator<Item = &Service> {
        self.services.values()
    }

    pub fn service(&self, name: &ServiceName) -> Option<&Service> {
        self.services.get(name)
    }

    pub fn aliases(&self) -> &BTreeMap<AliasName, ServiceName> {
        &self.aliases
    }

    pub fn profiles(&self) -> &BTreeMap<ProfileName, Vec<ServiceName>> {
        &self.profiles
    }

    pub fn upsert_service(&mut self, service: Service) -> Result<(), DomainError> {
        self.apply(|workspace| {
            workspace.services.insert(service.name.clone(), service);
        })
    }

    pub fn remove_service(&mut self, name: &ServiceName) -> Result<Service, DomainError> {
        if !self.services.contains_key(name) {
            return Err(self.unknown_target(name.as_str()));
        }
        let references = self.references_to(name);
        if !references.is_empty() {
            return Err(DomainError::ServiceInUse {
                service: name.clone(),
                references,
            });
        }
        self.services
            .remove(name)
            .ok_or_else(|| self.unknown_target(name.as_str()))
    }

    pub fn set_alias(&mut self, alias: AliasName, target: ServiceName) -> Result<(), DomainError> {
        self.apply(|workspace| {
            workspace.aliases.insert(alias, target);
        })
    }

    pub fn remove_alias(&mut self, alias: &AliasName) -> Result<ServiceName, DomainError> {
        self.aliases
            .remove(alias)
            .ok_or_else(|| DomainError::UnknownAlias(alias.to_string()))
    }

    pub fn set_profile(
        &mut self,
        name: ProfileName,
        members: Vec<ServiceName>,
    ) -> Result<(), DomainError> {
        self.apply(|workspace| {
            workspace.profiles.insert(name, unique(members));
        })
    }

    pub fn remove_profile(&mut self, name: &ProfileName) -> Result<Vec<ServiceName>, DomainError> {
        self.profiles
            .remove(name)
            .ok_or_else(|| self.unknown_profile(name.as_str()))
    }

    pub fn resolve_name(&self, raw: &str) -> Result<&Service, DomainError> {
        let by_service = || {
            ServiceName::parse(raw)
                .ok()
                .and_then(|name| self.services.get(&name))
        };
        let by_alias = || {
            AliasName::parse(raw)
                .ok()
                .and_then(|alias| self.aliases.get(&alias))
                .and_then(|name| self.services.get(name))
        };
        by_service()
            .or_else(by_alias)
            .ok_or_else(|| self.unknown_target(raw))
    }

    pub fn resolve_profile(&self, raw: &str) -> Result<&[ServiceName], DomainError> {
        ProfileName::parse(raw)
            .ok()
            .and_then(|name| self.profiles.get(&name))
            .map(Vec::as_slice)
            .ok_or_else(|| self.unknown_profile(raw))
    }

    fn apply(&mut self, change: impl FnOnce(&mut Self)) -> Result<(), DomainError> {
        let mut candidate = self.clone();
        change(&mut candidate);
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    fn validate(&self) -> Result<(), DomainError> {
        self.validate_needs()?;
        self.validate_acyclic()?;
        self.validate_aliases()?;
        self.validate_profiles()
    }

    fn validate_needs(&self) -> Result<(), DomainError> {
        for service in self.services.values() {
            if let Some(need) = service
                .needs
                .iter()
                .find(|need| !self.services.contains_key(*need))
            {
                return Err(DomainError::UnknownNeed {
                    service: service.name.clone(),
                    need: need.clone(),
                });
            }
        }
        Ok(())
    }

    fn validate_acyclic(&self) -> Result<(), DomainError> {
        let mut finished = BTreeSet::new();
        for start in self.services.keys() {
            let mut trail = Vec::new();
            if let Some(cycle) = self.find_cycle(start, &mut trail, &mut finished) {
                return Err(DomainError::DependencyCycle(cycle));
            }
        }
        Ok(())
    }

    fn find_cycle<'a>(
        &'a self,
        name: &'a ServiceName,
        trail: &mut Vec<&'a ServiceName>,
        finished: &mut BTreeSet<&'a ServiceName>,
    ) -> Option<Vec<ServiceName>> {
        if finished.contains(name) {
            return None;
        }
        if let Some(position) = trail.iter().position(|visited| *visited == name) {
            let mut cycle: Vec<ServiceName> = trail[position..]
                .iter()
                .map(|visited| (*visited).clone())
                .collect();
            cycle.push(name.clone());
            return Some(cycle);
        }
        trail.push(name);
        let needs = self
            .services
            .get(name)
            .map(|service| service.needs.as_slice())
            .unwrap_or_default();
        for need in needs {
            if let Some(cycle) = self.find_cycle(need, trail, finished) {
                return Some(cycle);
            }
        }
        trail.pop();
        finished.insert(name);
        None
    }

    fn validate_aliases(&self) -> Result<(), DomainError> {
        for (alias, target) in &self.aliases {
            if self
                .services
                .keys()
                .any(|name| name.as_str() == alias.as_str())
            {
                return Err(DomainError::AliasShadowsService(alias.clone()));
            }
            if !self.services.contains_key(target) {
                return Err(DomainError::UnknownAliasTarget {
                    alias: alias.clone(),
                    target: target.clone(),
                });
            }
        }
        Ok(())
    }

    fn validate_profiles(&self) -> Result<(), DomainError> {
        for (profile, members) in &self.profiles {
            if members.is_empty() {
                return Err(DomainError::EmptyProfile(profile.clone()));
            }
            if let Some(member) = members
                .iter()
                .find(|member| !self.services.contains_key(*member))
            {
                return Err(DomainError::UnknownProfileMember {
                    profile: profile.clone(),
                    member: member.clone(),
                });
            }
        }
        Ok(())
    }

    fn references_to(&self, name: &ServiceName) -> Vec<String> {
        let needed_by = self
            .services
            .values()
            .filter(|service| service.needs.contains(name))
            .map(|service| format!("service `{}`", service.name));
        let aliases = self
            .aliases
            .iter()
            .filter(|(_, target)| *target == name)
            .map(|(alias, _)| format!("alias `{alias}`"));
        let profiles = self
            .profiles
            .iter()
            .filter(|(_, members)| members.contains(name))
            .map(|(profile, _)| format!("profile `@{profile}`"));
        needed_by.chain(aliases).chain(profiles).collect()
    }

    fn unknown_target(&self, raw: &str) -> DomainError {
        let candidates = self
            .services
            .keys()
            .map(ServiceName::as_str)
            .chain(self.aliases.keys().map(AliasName::as_str));
        DomainError::UnknownTarget {
            name: raw.to_owned(),
            suggestion: closest(raw, candidates),
        }
    }

    fn unknown_profile(&self, raw: &str) -> DomainError {
        DomainError::UnknownProfile {
            name: raw.to_owned(),
            suggestion: closest(raw, self.profiles.keys().map(ProfileName::as_str)),
        }
    }
}

fn unique(members: Vec<ServiceName>) -> Vec<ServiceName> {
    let mut seen = BTreeSet::new();
    members
        .into_iter()
        .filter(|member| seen.insert(member.clone()))
        .collect()
}
