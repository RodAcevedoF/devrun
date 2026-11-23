#![allow(clippy::unwrap_used)]

mod common;

use common::{jaspy_like, name, port, root, service};
use devrun_domain::{
    Candidate, CommandLine, Confidence, DiscoveryDiff, RelativePath, RepoWorkspace, Runtime,
    Service, SharedPort, shared_ports,
};

fn candidate(service: Service) -> Candidate {
    Candidate::new(
        service,
        Confidence::High,
        "package.json script \"dev\": vite",
    )
}

fn names(candidates: &[Candidate]) -> Vec<&str> {
    candidates
        .iter()
        .map(|candidate| candidate.service.name.as_str())
        .collect()
}

fn unchanged_jaspy_candidates() -> Vec<Candidate> {
    jaspy_like()
        .services()
        .map(|service| candidate(service.clone()))
        .collect()
}

#[test]
fn confidence_is_displayed_in_lowercase() {
    let shown: Vec<String> = [Confidence::High, Confidence::Medium, Confidence::Low]
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(shown, ["high", "medium", "low"]);
}

#[test]
fn candidates_matching_the_config_produce_an_empty_diff() {
    let diff = DiscoveryDiff::between(&jaspy_like(), &unchanged_jaspy_candidates());
    assert!(diff.is_empty());
    assert_eq!(diff, DiscoveryDiff::default());
}

#[test]
fn unknown_candidates_are_added_sorted_by_name() {
    let mut candidates = unchanged_jaspy_candidates();
    candidates.insert(0, candidate(service("worker")));
    candidates.push(candidate(service("brief")));
    let diff = DiscoveryDiff::between(&jaspy_like(), &candidates);
    assert_eq!(names(&diff.added), ["brief", "worker"]);
    assert!(diff.changed.is_empty());
    assert!(diff.missing.is_empty());
    assert!(!diff.is_empty());
}

#[test]
fn candidates_differing_in_runtime_path_or_command_are_changed() {
    let mut candidates = unchanged_jaspy_candidates();
    for candidate in &mut candidates {
        match candidate.service.name.as_str() {
            "web" => candidate.service.command = CommandLine::parse("npm run start").unwrap(),
            "api" => candidate.service.path = RelativePath::parse("apps/backend").unwrap(),
            "mongo" => candidate.service.runtime = Runtime::Compose,
            _ => {}
        }
    }
    let diff = DiscoveryDiff::between(&jaspy_like(), &candidates);
    let changed: Vec<(&str, &str)> = diff
        .changed
        .iter()
        .map(|change| {
            (
                change.current.name.as_str(),
                change.candidate.service.name.as_str(),
            )
        })
        .collect();
    assert_eq!(
        changed,
        [("api", "api"), ("mongo", "mongo"), ("web", "web")]
    );
    let web = &diff.changed[2];
    assert_eq!(web.current.command.as_str(), "npm run dev");
    assert_eq!(web.candidate.service.command.as_str(), "npm run start");
    assert!(diff.added.is_empty());
    assert!(diff.missing.is_empty());
}

fn changed_fields(diff: &DiscoveryDiff) -> Vec<(&str, [bool; 6])> {
    diff.changed
        .iter()
        .map(|change| {
            (
                change.current.name.as_str(),
                [
                    change.runtime_changed(),
                    change.path_changed(),
                    change.command_changed(),
                    change.ports_changed(),
                    change.needs_changed(),
                    change.prepare_changed(),
                ],
            )
        })
        .collect()
}

#[test]
fn candidates_differing_in_ports_needs_or_prepare_are_changed() {
    let mut candidates = unchanged_jaspy_candidates();
    for candidate in &mut candidates {
        let service = &mut candidate.service;
        match service.name.as_str() {
            "web" => service.ports = vec![port(5174)],
            "knowledge-graph" => service.needs = vec![name("mongo")],
            "api" => service.prepare = vec![CommandLine::parse("npm run build").unwrap()],
            _ => {}
        }
    }
    let diff = DiscoveryDiff::between(&jaspy_like(), &candidates);
    assert_eq!(
        changed_fields(&diff),
        [
            ("api", [false, false, false, false, false, true]),
            ("knowledge-graph", [false, false, false, false, true, false]),
            ("web", [false, false, false, true, false, false]),
        ]
    );
    assert!(diff.added.is_empty());
    assert!(diff.missing.is_empty());
}

#[test]
fn undiscovered_ports_needs_and_prepare_keep_the_configured_values() {
    let mut candidates = unchanged_jaspy_candidates();
    for candidate in &mut candidates {
        candidate.service.ports.clear();
        candidate.service.needs.clear();
        candidate.service.prepare.clear();
    }
    let diff = DiscoveryDiff::between(&jaspy_like(), &candidates);
    assert_eq!(diff, DiscoveryDiff::default());
}

#[test]
fn ports_and_needs_are_compared_regardless_of_order() {
    let mut workspace = jaspy_like();
    workspace
        .upsert_service(service("web").with_ports([port(5173), port(5174)]))
        .unwrap();
    let mut candidates = unchanged_jaspy_candidates();
    for candidate in &mut candidates {
        let service = &mut candidate.service;
        match service.name.as_str() {
            "web" => service.ports = vec![port(5174), port(5173)],
            "knowledge-graph" => service.needs = vec![name("neo4j"), name("mongo")],
            _ => {}
        }
    }
    let diff = DiscoveryDiff::between(&workspace, &candidates);
    assert_eq!(diff, DiscoveryDiff::default());
}

#[test]
fn configured_services_not_discovered_are_missing_except_custom_ones() {
    let mut workspace = jaspy_like();
    workspace
        .upsert_service(Service::new(
            name("tunnel"),
            Runtime::Custom,
            RelativePath::repository_root(),
            CommandLine::parse("ngrok http 3000").unwrap(),
        ))
        .unwrap();
    let candidates: Vec<Candidate> = unchanged_jaspy_candidates()
        .into_iter()
        .filter(|candidate| !["web", "neo4j"].contains(&candidate.service.name.as_str()))
        .collect();
    let diff = DiscoveryDiff::between(&workspace, &candidates);
    assert_eq!(diff.missing, [name("neo4j"), name("web")]);
    assert!(diff.added.is_empty());
    assert!(diff.changed.is_empty());
}

#[test]
fn an_empty_workspace_reports_every_candidate_as_added() {
    let workspace = RepoWorkspace::new(root()).unwrap();
    let diff = DiscoveryDiff::between(&workspace, &unchanged_jaspy_candidates());
    assert_eq!(
        names(&diff.added),
        ["api", "knowledge-graph", "mongo", "neo4j", "web"]
    );
}

#[test]
fn ports_claimed_by_several_candidates_are_reported_in_port_order() {
    let candidates = [
        candidate(service("research").with_ports([port(8080)])),
        candidate(service("graph").with_ports([port(8080), port(3000)])),
        candidate(service("feeder").with_ports([port(3000)])),
        candidate(service("web").with_ports([port(5173)])),
    ];
    assert_eq!(
        shared_ports(&candidates),
        [
            SharedPort {
                port: port(3000),
                services: vec![name("feeder"), name("graph")],
            },
            SharedPort {
                port: port(8080),
                services: vec![name("graph"), name("research")],
            },
        ]
    );
}

fn dir(raw: &str) -> RelativePath {
    RelativePath::parse(raw).unwrap()
}

fn watched(workspace: &RepoWorkspace, raw: &str) -> RepoWorkspace {
    let mut workspace = workspace.clone();
    let api = workspace.service(&name(raw)).unwrap().clone();
    workspace
        .upsert_service(api.with_watch([dir("packages/a"), dir("packages/b")]))
        .unwrap();
    workspace
}

#[test]
fn watch_dirs_start_empty_and_are_kept_sorted_and_unique() {
    assert!(service("api").watch.is_empty());
    let api =
        service("api").with_watch([dir("packages/b"), dir("./packages/a/"), dir("packages/b")]);
    assert_eq!(api.watch, [dir("packages/a"), dir("packages/b")]);
}

#[test]
fn candidates_differing_in_watch_dirs_are_changed() {
    let mut candidates = unchanged_jaspy_candidates();
    for candidate in &mut candidates {
        if candidate.service.name.as_str() == "api" {
            candidate.service.watch = vec![dir("packages/a"), dir("packages/b")];
        }
    }
    let diff = DiscoveryDiff::between(&jaspy_like(), &candidates);
    let changed: Vec<(&str, bool, bool)> = diff
        .changed
        .iter()
        .map(|change| {
            (
                change.current.name.as_str(),
                change.watch_changed(),
                change.prepare_changed(),
            )
        })
        .collect();
    assert_eq!(changed, [("api", true, false)]);
    assert!(diff.added.is_empty());
    assert!(diff.missing.is_empty());
}

#[test]
fn matching_watch_dirs_are_compared_regardless_of_order() {
    let workspace = watched(&jaspy_like(), "api");
    let mut candidates: Vec<Candidate> = workspace
        .services()
        .map(|service| candidate(service.clone()))
        .collect();
    for candidate in &mut candidates {
        candidate.service.watch.reverse();
    }
    let diff = DiscoveryDiff::between(&workspace, &candidates);
    assert_eq!(diff, DiscoveryDiff::default());
}

#[test]
fn undiscovered_watch_dirs_keep_the_configured_values() {
    let workspace = watched(&jaspy_like(), "api");
    let diff = DiscoveryDiff::between(&workspace, &unchanged_jaspy_candidates());
    assert_eq!(diff, DiscoveryDiff::default());
}
