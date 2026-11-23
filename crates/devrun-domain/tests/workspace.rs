#![allow(clippy::unwrap_used)]

mod common;

use std::path::PathBuf;

use common::{alias, jaspy_like, name, profile, root, service, service_needing};
use devrun_domain::{DomainError, RepoWorkspace};

fn build(services: Vec<devrun_domain::Service>) -> Result<RepoWorkspace, DomainError> {
    RepoWorkspace::from_parts(root(), services, [], [])
}

#[test]
fn root_must_be_absolute() {
    assert_eq!(
        RepoWorkspace::new(PathBuf::from("repo"))
            .unwrap_err()
            .to_string(),
        "repository root `repo` must be an absolute path"
    );
}

#[test]
fn duplicate_services_are_rejected() {
    assert_eq!(
        build(vec![service("web"), service("web")])
            .unwrap_err()
            .to_string(),
        "service `web` is defined more than once"
    );
}

#[test]
fn needs_must_reference_existing_services() {
    assert_eq!(
        build(vec![service_needing("kg", &["mongo"])])
            .unwrap_err()
            .to_string(),
        "service `kg` needs unknown service `mongo`"
    );
}

#[test]
fn dependency_cycles_are_rejected_with_their_path() {
    let error = build(vec![
        service_needing("a", &["b"]),
        service_needing("b", &["a"]),
    ])
    .unwrap_err();
    assert_eq!(error.to_string(), "dependency cycle: a -> b -> a");
    let error = build(vec![service_needing("a", &["a"])]).unwrap_err();
    assert_eq!(error.to_string(), "dependency cycle: a -> a");
}

#[test]
fn aliases_cannot_shadow_services_or_point_nowhere() {
    let mut workspace = jaspy_like();
    assert_eq!(
        workspace
            .set_alias(alias("api"), name("web"))
            .unwrap_err()
            .to_string(),
        "alias `api` has the same name as a service"
    );
    assert_eq!(
        workspace
            .set_alias(alias("x"), name("ghost"))
            .unwrap_err()
            .to_string(),
        "alias `x` points to unknown service `ghost`"
    );
    assert_eq!(workspace, jaspy_like());
}

#[test]
fn profiles_must_be_non_empty_and_reference_existing_services() {
    let mut workspace = jaspy_like();
    assert_eq!(
        workspace
            .set_profile(profile("empty"), vec![])
            .unwrap_err()
            .to_string(),
        "profile `@empty` must contain at least one service"
    );
    assert_eq!(
        workspace
            .set_profile(profile("data"), vec![name("api"), name("ghost")])
            .unwrap_err()
            .to_string(),
        "profile `@data` includes unknown service `ghost`"
    );
    assert_eq!(workspace, jaspy_like());
}

#[test]
fn profile_members_are_deduplicated_preserving_order() {
    let mut workspace = jaspy_like();
    workspace
        .set_profile(profile("data"), vec![name("api"), name("web"), name("api")])
        .unwrap();
    assert_eq!(
        workspace.profiles()[&profile("data")],
        vec![name("api"), name("web")]
    );
}

#[test]
fn removing_a_referenced_service_is_rejected_and_leaves_the_workspace_unchanged() {
    let mut workspace = jaspy_like();
    assert_eq!(
        workspace
            .remove_service(&name("web"))
            .unwrap_err()
            .to_string(),
        "service `web` is still used by alias `fe`, profile `@fullstack`"
    );
    assert_eq!(
        workspace
            .remove_service(&name("mongo"))
            .unwrap_err()
            .to_string(),
        "service `mongo` is still used by service `knowledge-graph`"
    );
    assert_eq!(workspace, jaspy_like());
}

#[test]
fn unreferenced_services_can_be_removed() {
    let mut workspace = jaspy_like();
    workspace.upsert_service(service("docs")).unwrap();
    assert_eq!(
        workspace.remove_service(&name("docs")).unwrap().name,
        name("docs")
    );
    assert_eq!(workspace, jaspy_like());
}

#[test]
fn rejected_upserts_leave_the_workspace_unchanged() {
    let mut workspace = jaspy_like();
    let error = workspace
        .upsert_service(service_needing("mongo", &["knowledge-graph"]))
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "dependency cycle: knowledge-graph -> mongo -> knowledge-graph"
    );
    assert_eq!(workspace, jaspy_like());
}

#[test]
fn names_resolve_through_services_and_aliases() {
    let workspace = jaspy_like();
    assert_eq!(workspace.resolve_name("web").unwrap().name, name("web"));
    assert_eq!(
        workspace.resolve_name("kg").unwrap().name,
        name("knowledge-graph")
    );
}

#[test]
fn unknown_names_suggest_the_closest_match() {
    let workspace = jaspy_like();
    assert_eq!(
        workspace.resolve_name("wbe").unwrap_err().to_string(),
        "unknown service or alias `wbe` (did you mean `web`?)"
    );
    assert_eq!(
        workspace.resolve_name("zzz").unwrap_err().to_string(),
        "unknown service or alias `zzz`"
    );
    assert_eq!(
        workspace
            .resolve_profile("fulstack")
            .unwrap_err()
            .to_string(),
        "unknown profile `@fulstack` (did you mean `fullstack`?)"
    );
}
