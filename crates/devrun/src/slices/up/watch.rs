use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use devrun_domain::RunPlan;
use devrun_supervisor::{Command, CommandSender, ServiceId, SupervisorError};
use notify::{Config, Event, EventKind, EventKindMask, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep_until};

use super::launch;

const QUIET_WINDOW: Duration = Duration::from_millis(300);
const IGNORED_DIRS: [&str; 8] = [
    "node_modules",
    "dist",
    "build",
    "out",
    ".next",
    ".turbo",
    "coverage",
    ".git",
];
const IGNORED_EXTENSION: &str = "tsbuildinfo";

type Services = BTreeSet<ServiceId>;

pub struct WatchTargets(BTreeMap<PathBuf, Services>);

impl WatchTargets {
    pub fn from_plan(plan: &RunPlan, root: &Path) -> Self {
        let targets = plan
            .services()
            .iter()
            .flat_map(|planned| {
                let id = launch::service_id(&planned.service.name);
                planned
                    .service
                    .watch
                    .iter()
                    .filter_map(|dir| dir.within(root).canonicalize().ok())
                    .map(move |dir| (dir, id.clone()))
            })
            .fold(
                BTreeMap::<PathBuf, Services>::new(),
                |mut targets, (dir, id)| {
                    targets.entry(dir).or_default().insert(id);
                    targets
                },
            );
        Self(targets)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn dirs(&self) -> Vec<PathBuf> {
        self.0.keys().cloned().collect()
    }

    fn services_for(&self, path: &Path) -> Services {
        self.0
            .iter()
            .filter(|(dir, _)| {
                path.strip_prefix(dir)
                    .is_ok_and(|relative| !is_ignored(relative))
            })
            .flat_map(|(_, services)| services.iter().cloned())
            .collect()
    }

    fn affected_by(&self, event: &Event) -> Services {
        if !counts(event.kind) {
            return Services::new();
        }
        event
            .paths
            .iter()
            .flat_map(|path| self.services_for(path))
            .collect()
    }
}

fn counts(kind: EventKind) -> bool {
    match kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => true,
        EventKind::Any | EventKind::Access(_) | EventKind::Other => false,
    }
}

fn is_ignored(relative: &Path) -> bool {
    relative
        .extension()
        .is_some_and(|extension| extension == IGNORED_EXTENSION)
        || relative.components().any(|component| {
            component
                .as_os_str()
                .to_str()
                .is_some_and(|name| IGNORED_DIRS.contains(&name))
        })
}

#[derive(Default)]
struct Debouncer {
    pending: Services,
    deadline: Option<Instant>,
}

impl Debouncer {
    fn push(&mut self, services: Services, now: Instant) {
        if services.is_empty() {
            return;
        }
        self.pending.extend(services);
        self.deadline = Some(now + QUIET_WINDOW);
    }

    fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    fn take_due(&mut self, now: Instant) -> Option<Services> {
        let due = self.deadline.is_some_and(|deadline| now >= deadline);
        due.then(|| {
            self.deadline = None;
            std::mem::take(&mut self.pending)
        })
    }
}

pub struct Watch {
    watcher: RecommendedWatcher,
    changes: UnboundedReceiver<Services>,
}

impl Watch {
    pub fn start(targets: WatchTargets) -> notify::Result<Option<Self>> {
        if targets.is_empty() {
            return Ok(None);
        }
        let dirs = targets.dirs();
        let (changes_sender, changes) = mpsc::unbounded_channel();
        let config = Config::default().with_event_kinds(EventKindMask::CORE);
        let mut watcher =
            RecommendedWatcher::new(forward_changes(targets, changes_sender), config)?;
        for dir in &dirs {
            watcher.watch(dir, RecursiveMode::Recursive)?;
        }
        Ok(Some(Self { watcher, changes }))
    }

    pub fn rebuild_through(self, commands: CommandSender) -> Rebuilds {
        Rebuilds {
            _watcher: self.watcher,
            task: tokio::spawn(debounce_into_rebuilds(self.changes, commands)),
        }
    }
}

fn forward_changes(
    targets: WatchTargets,
    changes: UnboundedSender<Services>,
) -> impl FnMut(notify::Result<Event>) + Send + 'static {
    move |event| {
        let services = event
            .map(|event| targets.affected_by(&event))
            .unwrap_or_default();
        if !services.is_empty() {
            let _ = changes.send(services);
        }
    }
}

async fn debounce_into_rebuilds(mut changes: UnboundedReceiver<Services>, commands: CommandSender) {
    let mut debouncer = Debouncer::default();
    loop {
        tokio::select! {
            change = changes.recv() => match change {
                Some(services) => debouncer.push(services, Instant::now()),
                None => return,
            },
            () = quiet_until(debouncer.deadline()) => {
                let due = debouncer.take_due(Instant::now()).unwrap_or_default();
                if send_rebuilds(&commands, due).is_err() {
                    return;
                }
            }
        }
    }
}

async fn quiet_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn send_rebuilds(commands: &CommandSender, services: Services) -> Result<(), SupervisorError> {
    services
        .into_iter()
        .try_for_each(|service| commands.send(Command::Rebuild(service)))
}

pub struct Rebuilds {
    _watcher: RecommendedWatcher,
    task: JoinHandle<()>,
}

impl Drop for Rebuilds {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use devrun_domain::{
        CommandLine, RelativePath, RepoWorkspace, RunPlan, Runtime, Service, ServiceName,
        TargetExpr,
    };
    use devrun_supervisor::ServiceId;
    use notify::event::{AccessKind, CreateKind, ModifyKind, RemoveKind};
    use notify::{Event, EventKind};
    use tokio::time::Instant;

    use super::*;

    fn ids(raw: &[&str]) -> BTreeSet<ServiceId> {
        raw.iter().map(|id| ServiceId::new(*id)).collect()
    }

    fn service(name: &str, watch: &[&str]) -> Service {
        Service::new(
            ServiceName::parse(name).unwrap(),
            Runtime::Custom,
            RelativePath::parse("apps/web").unwrap(),
            CommandLine::parse("true").unwrap(),
        )
        .with_watch(watch.iter().map(|dir| RelativePath::parse(dir).unwrap()))
    }

    fn targets_for(root: &Path, services: Vec<Service>, targets: &[&str]) -> WatchTargets {
        let workspace = RepoWorkspace::from_parts(root.to_path_buf(), services, [], []).unwrap();
        let plan = RunPlan::resolve(&workspace, &TargetExpr::parse(targets).unwrap()).unwrap();
        WatchTargets::from_plan(&plan, root)
    }

    fn repo_with_dirs(dirs: &[&str]) -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("repo");
        for dir in dirs {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        (home, root)
    }

    fn canonical(root: &Path, relative: &str) -> PathBuf {
        fs::canonicalize(root.join(relative)).unwrap()
    }

    #[test]
    fn every_build_output_and_tooling_dir_is_ignored_at_any_depth() {
        for name in [
            "node_modules",
            "dist",
            "build",
            "out",
            ".next",
            ".turbo",
            "coverage",
            ".git",
        ] {
            assert!(is_ignored(Path::new(name)), "{name}");
            assert!(is_ignored(&Path::new(name).join("index.js")), "{name}");
            assert!(
                is_ignored(&Path::new("src/nested").join(name).join("chunk.js")),
                "{name}"
            );
        }
    }

    #[test]
    fn tsbuildinfo_files_are_ignored_anywhere() {
        assert!(is_ignored(Path::new("tsconfig.tsbuildinfo")));
        assert!(is_ignored(Path::new("src/tsconfig.app.tsbuildinfo")));
    }

    #[test]
    fn source_files_are_not_ignored() {
        assert!(!is_ignored(Path::new("src/index.ts")));
        assert!(!is_ignored(Path::new("package.json")));
        assert!(!is_ignored(Path::new("src/distance.ts")));
        assert!(!is_ignored(Path::new("src/builder/output.ts")));
        assert!(!is_ignored(Path::new("")));
    }

    #[test]
    fn a_repo_living_under_a_dir_named_build_still_sees_its_sources() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("build/repo");
        fs::create_dir_all(root.join("packages/ui/src")).unwrap();
        let targets = targets_for(&root, vec![service("web", &["packages/ui"])], &["web"]);

        let source = canonical(&root, "packages/ui/src").join("button.ts");
        let output = canonical(&root, "packages/ui").join("dist/button.js");

        assert_eq!(targets.services_for(&source), ids(&["web"]));
        assert_eq!(targets.services_for(&output), ids(&[]));
    }

    #[test]
    fn paths_map_to_every_service_whose_watch_dir_contains_them() {
        let (_home, root) = repo_with_dirs(&["packages/ui/src/forms", "packages/core/src"]);
        let targets = targets_for(
            &root,
            vec![
                service("web", &["packages/ui", "packages/core"]),
                service("admin", &["packages/ui"]),
                service("api", &["packages/core"]),
            ],
            &["web", "admin", "api"],
        );
        let ui = canonical(&root, "packages/ui");
        let core = canonical(&root, "packages/core");

        assert_eq!(
            targets.services_for(&ui.join("src/forms/input.tsx")),
            ids(&["admin", "web"])
        );
        assert_eq!(
            targets.services_for(&core.join("src/index.ts")),
            ids(&["api", "web"])
        );
        assert_eq!(
            targets.services_for(&canonical(&root, "packages").join("README.md")),
            ids(&[])
        );
        assert_eq!(
            targets.services_for(&ui.join("node_modules/react/index.js")),
            ids(&[])
        );
    }

    #[test]
    fn nested_watch_dirs_of_different_services_both_see_a_deep_change() {
        let (_home, root) = repo_with_dirs(&["packages/ui/src/forms"]);
        let targets = targets_for(
            &root,
            vec![
                service("web", &["packages/ui"]),
                service("admin", &["packages/ui/src"]),
            ],
            &["web", "admin"],
        );
        let forms = canonical(&root, "packages/ui/src/forms");

        assert_eq!(
            targets.services_for(&forms.join("input.tsx")),
            ids(&["admin", "web"])
        );
        assert_eq!(
            targets.services_for(&canonical(&root, "packages/ui").join("package.json")),
            ids(&["web"])
        );
    }

    #[test]
    fn watch_dirs_are_canonical_skip_missing_dirs_and_only_cover_selected_services() {
        let (_home, root) = repo_with_dirs(&["packages/ui", "packages/core"]);
        let targets = targets_for(
            &root,
            vec![
                service("web", &["packages/ui", "packages/missing"]),
                service("api", &["packages/core"]),
            ],
            &["web"],
        );

        assert_eq!(targets.dirs(), vec![canonical(&root, "packages/ui")]);
    }

    #[test]
    fn no_watch_dirs_means_no_watcher() {
        let (_home, root) = repo_with_dirs(&["packages/ui"]);
        let unwatched = targets_for(&root, vec![service("web", &[])], &["web"]);
        let only_missing = targets_for(&root, vec![service("web", &["gone"])], &["web"]);

        assert!(unwatched.is_empty());
        assert!(only_missing.is_empty());
        assert!(Watch::start(unwatched).unwrap().is_none());
        assert!(Watch::start(only_missing).unwrap().is_none());
    }

    #[test]
    fn only_create_modify_and_remove_events_affect_services() {
        let (_home, root) = repo_with_dirs(&["packages/ui/src"]);
        let targets = targets_for(&root, vec![service("web", &["packages/ui"])], &["web"]);
        let source = canonical(&root, "packages/ui/src").join("index.ts");
        let event = |kind| Event::new(kind).add_path(source.clone());

        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Remove(RemoveKind::File),
        ] {
            assert_eq!(targets.affected_by(&event(kind)), ids(&["web"]), "{kind:?}");
        }
        for kind in [
            EventKind::Access(AccessKind::Any),
            EventKind::Any,
            EventKind::Other,
        ] {
            assert_eq!(targets.affected_by(&event(kind)), ids(&[]), "{kind:?}");
        }
    }

    #[test]
    fn an_event_with_several_paths_affects_the_union_of_their_services() {
        let (_home, root) = repo_with_dirs(&["packages/ui", "packages/core"]);
        let targets = targets_for(
            &root,
            vec![
                service("web", &["packages/ui"]),
                service("api", &["packages/core"]),
            ],
            &["web", "api"],
        );
        let event = Event::new(EventKind::Modify(ModifyKind::Any))
            .add_path(canonical(&root, "packages/ui").join("a.ts"))
            .add_path(canonical(&root, "packages/core").join("dist/b.js"))
            .add_path(canonical(&root, "packages/core").join("b.ts"));

        assert_eq!(targets.affected_by(&event), ids(&["api", "web"]));
    }

    #[test]
    fn a_burst_inside_the_quiet_window_becomes_one_batch() {
        let start = Instant::now();
        let mut debouncer = Debouncer::default();
        let at = |millis| start + Duration::from_millis(millis);

        debouncer.push(ids(&["web"]), at(0));
        debouncer.push(ids(&["api"]), at(200));
        debouncer.push(ids(&["web"]), at(450));

        assert_eq!(debouncer.deadline(), Some(at(450) + QUIET_WINDOW));
        assert_eq!(debouncer.take_due(at(700)), None);
        assert_eq!(debouncer.take_due(at(750)), Some(ids(&["api", "web"])));
        assert_eq!(debouncer.deadline(), None);
        assert_eq!(debouncer.take_due(at(2000)), None);
    }

    #[test]
    fn bursts_separated_by_more_than_the_quiet_window_become_two_batches() {
        let start = Instant::now();
        let mut debouncer = Debouncer::default();
        let at = |millis| start + Duration::from_millis(millis);

        debouncer.push(ids(&["web"]), at(0));
        debouncer.push(ids(&["web"]), at(100));
        let first = debouncer.take_due(at(400));
        debouncer.push(ids(&["api"]), at(401));
        let early = debouncer.take_due(at(700));
        let second = debouncer.take_due(at(701));

        assert_eq!(first, Some(ids(&["web"])));
        assert_eq!(early, None);
        assert_eq!(second, Some(ids(&["api"])));
    }

    #[test]
    fn the_quiet_window_is_300_ms() {
        assert_eq!(QUIET_WINDOW, Duration::from_millis(300));
    }

    #[test]
    fn pushing_no_services_does_not_open_or_extend_the_window() {
        let start = Instant::now();
        let mut debouncer = Debouncer::default();

        debouncer.push(ids(&[]), start);
        assert_eq!(debouncer.deadline(), None);
        debouncer.push(ids(&["web"]), start);
        debouncer.push(ids(&[]), start + Duration::from_millis(200));

        assert_eq!(debouncer.deadline(), Some(start + QUIET_WINDOW));
    }
}
