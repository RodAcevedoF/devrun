use std::collections::{HashMap, HashSet};
use std::time::Duration;

use sysinfo::{MINIMUM_CPU_UPDATE_INTERVAL, Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tokio::time::{Instant, Interval, MissedTickBehavior, interval_at};

use crate::ResourceUsage;

pub(crate) struct UsageSampler {
    system: System,
    every: Duration,
}

impl UsageSampler {
    pub fn new(interval: Option<Duration>) -> Option<Self> {
        let every = interval?.max(MINIMUM_CPU_UPDATE_INTERVAL);
        let system = System::new().ok()?;
        Some(Self { system, every })
    }

    pub fn clock(&self) -> Interval {
        let mut clock = interval_at(Instant::now() + self.every, self.every);
        clock.set_missed_tick_behavior(MissedTickBehavior::Delay);
        clock
    }

    pub fn sample(&mut self, leaders: &[u32]) -> Vec<Option<ResourceUsage>> {
        if leaders.is_empty() {
            return Vec::new();
        }
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().without_tasks(),
        );
        let forest = ProcessForest::new(
            self.system
                .processes()
                .iter()
                .map(|(pid, process)| (pid.as_u32(), process.parent().map(Pid::as_u32))),
        );
        let trees: Vec<Vec<u32>> = leaders.iter().map(|&leader| forest.tree(leader)).collect();
        let members: Vec<Pid> = trees.iter().flatten().copied().map(Pid::from_u32).collect();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&members),
            true,
            ProcessRefreshKind::nothing()
                .without_tasks()
                .with_cpu()
                .with_memory(),
        );
        trees.iter().map(|tree| self.measure(tree)).collect()
    }

    fn measure(&self, tree: &[u32]) -> Option<ResourceUsage> {
        let processes: Vec<_> = tree
            .iter()
            .filter_map(|&pid| self.system.process(Pid::from_u32(pid)))
            .collect();
        if processes.is_empty() {
            return None;
        }
        let cpu_percent: f64 = processes
            .iter()
            .map(|process| f64::from(process.cpu_usage()))
            .sum();
        Some(ResourceUsage {
            cpu_permille: permille(cpu_percent),
            rss_bytes: processes.iter().map(|process| process.memory()).sum(),
            processes: u32::try_from(processes.len()).unwrap_or(u32::MAX),
        })
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn permille(percent: f64) -> u32 {
    (percent * 10.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

pub(crate) async fn tick(clock: Option<&mut Interval>) {
    match clock {
        Some(clock) => {
            clock.tick().await;
        }
        None => std::future::pending().await,
    }
}

struct ProcessForest {
    known: HashSet<u32>,
    children: HashMap<u32, Vec<u32>>,
}

impl ProcessForest {
    fn new(parents: impl IntoIterator<Item = (u32, Option<u32>)>) -> Self {
        let mut known = HashSet::new();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, parent) in parents {
            known.insert(pid);
            if let Some(parent) = parent {
                children.entry(parent).or_default().push(pid);
            }
        }
        Self { known, children }
    }

    fn tree(&self, leader: u32) -> Vec<u32> {
        if !self.known.contains(&leader) {
            return Vec::new();
        }
        let mut seen = HashSet::from([leader]);
        let mut tree = vec![leader];
        let mut next = 0;
        while let Some(&pid) = tree.get(next) {
            for &child in self.children.get(&pid).into_iter().flatten() {
                if seen.insert(child) {
                    tree.push(child);
                }
            }
            next += 1;
        }
        tree
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(mut pids: Vec<u32>) -> Vec<u32> {
        pids.sort_unstable();
        pids
    }

    fn forest() -> ProcessForest {
        ProcessForest::new([
            (1, None),
            (10, Some(1)),
            (11, Some(10)),
            (12, Some(10)),
            (13, Some(12)),
            (20, Some(1)),
            (21, Some(20)),
        ])
    }

    #[test]
    fn a_tree_holds_the_leader_and_every_descendant() {
        assert_eq!(sorted(forest().tree(10)), vec![10, 11, 12, 13]);
    }

    #[test]
    fn a_tree_excludes_unrelated_processes() {
        let tree = forest().tree(20);

        assert_eq!(sorted(tree), vec![20, 21]);
    }

    #[test]
    fn a_missing_leader_yields_no_tree() {
        assert_eq!(forest().tree(99), Vec::<u32>::new());
    }

    #[test]
    fn a_leader_without_children_is_a_tree_of_one() {
        assert_eq!(forest().tree(13), vec![13]);
    }

    #[test]
    fn a_parent_cycle_does_not_loop_forever() {
        let forest = ProcessForest::new([(5, Some(6)), (6, Some(5))]);

        assert_eq!(sorted(forest.tree(5)), vec![5, 6]);
    }

    #[test]
    fn cpu_percent_becomes_rounded_permille() {
        assert_eq!(permille(0.0), 0);
        assert_eq!(permille(12.54), 125);
        assert_eq!(permille(-1.0), 0);
        assert_eq!(permille(200.0), 2000);
    }
}
