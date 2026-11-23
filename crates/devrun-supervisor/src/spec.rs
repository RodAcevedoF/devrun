use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{ServiceId, SupervisorError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub id: ServiceId,
    pub cwd: PathBuf,
    pub command: String,
    pub env: BTreeMap<String, String>,
    pub path_prefix: Vec<PathBuf>,
    pub prepare: Vec<String>,
    pub ports: Vec<u16>,
    pub needs: Vec<ServiceId>,
}

impl LaunchSpec {
    pub fn new(id: ServiceId, cwd: impl AsRef<Path>, command: impl Into<String>) -> Self {
        Self {
            id,
            cwd: cwd.as_ref().to_path_buf(),
            command: command.into(),
            env: BTreeMap::new(),
            path_prefix: Vec::new(),
            prepare: Vec::new(),
            ports: Vec::new(),
            needs: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorOptions {
    pub grace_period: Duration,
    pub health_interval: Duration,
    pub rows: u16,
    pub cols: u16,
    pub output_buffer_bytes: usize,
    pub usage_interval: Option<Duration>,
}

impl Default for SupervisorOptions {
    fn default() -> Self {
        Self {
            grace_period: Duration::from_secs(5),
            health_interval: Duration::from_millis(500),
            rows: 24,
            cols: 80,
            output_buffer_bytes: 1024 * 1024,
            usage_interval: Some(Duration::from_secs(2)),
        }
    }
}

pub(crate) struct LaunchPlan {
    pub specs: Vec<LaunchSpec>,
    pub needs: Vec<Vec<usize>>,
    pub start_order: Vec<usize>,
}

impl LaunchPlan {
    pub fn validate(specs: Vec<LaunchSpec>) -> Result<Self, SupervisorError> {
        let index = index_by_id(&specs)?;
        let needs = resolve_needs(&specs, &index)?;
        let start_order = DependencyWalk::new(&specs, &needs).start_order()?;
        Ok(Self {
            specs,
            needs,
            start_order,
        })
    }
}

fn index_by_id(specs: &[LaunchSpec]) -> Result<HashMap<&ServiceId, usize>, SupervisorError> {
    let mut index = HashMap::new();
    for (position, spec) in specs.iter().enumerate() {
        if index.insert(&spec.id, position).is_some() {
            return Err(SupervisorError::DuplicateService(spec.id.clone()));
        }
    }
    Ok(index)
}

fn resolve_needs(
    specs: &[LaunchSpec],
    index: &HashMap<&ServiceId, usize>,
) -> Result<Vec<Vec<usize>>, SupervisorError> {
    specs
        .iter()
        .map(|spec| {
            spec.needs
                .iter()
                .map(|need| {
                    index
                        .get(need)
                        .copied()
                        .ok_or_else(|| SupervisorError::UnknownNeed {
                            service: spec.id.clone(),
                            need: need.clone(),
                        })
                })
                .collect()
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Unvisited,
    InProgress,
    Done,
}

struct DependencyWalk<'a> {
    specs: &'a [LaunchSpec],
    needs: &'a [Vec<usize>],
    marks: Vec<Mark>,
    path: Vec<usize>,
    order: Vec<usize>,
}

impl<'a> DependencyWalk<'a> {
    fn new(specs: &'a [LaunchSpec], needs: &'a [Vec<usize>]) -> Self {
        Self {
            specs,
            needs,
            marks: vec![Mark::Unvisited; specs.len()],
            path: Vec::new(),
            order: Vec::with_capacity(specs.len()),
        }
    }

    fn start_order(mut self) -> Result<Vec<usize>, SupervisorError> {
        for service in 0..self.specs.len() {
            self.visit(service)?;
        }
        Ok(self.order)
    }

    fn visit(&mut self, service: usize) -> Result<(), SupervisorError> {
        match self.marks[service] {
            Mark::Done => return Ok(()),
            Mark::InProgress => return Err(self.cycle_through(service)),
            Mark::Unvisited => {}
        }
        self.marks[service] = Mark::InProgress;
        self.path.push(service);
        for &need in &self.needs[service] {
            self.visit(need)?;
        }
        self.path.pop();
        self.marks[service] = Mark::Done;
        self.order.push(service);
        Ok(())
    }

    fn cycle_through(&self, service: usize) -> SupervisorError {
        let start = self
            .path
            .iter()
            .position(|&step| step == service)
            .unwrap_or_default();
        let cycle = self.path[start..]
            .iter()
            .chain(std::iter::once(&service))
            .map(|&step| self.specs[step].id.clone())
            .collect();
        SupervisorError::DependencyCycle(cycle)
    }
}
