#![allow(clippy::unwrap_used)]

mod common;

use common::{jaspy_like, name, port, root, service, service_needing};
use devrun_domain::{
    PlanReason, RepoWorkspace, RunPlan, ServiceName, SkippedNeed, TargetExpr, TargetOp, TargetRef,
};

fn plan(workspace: &RepoWorkspace, tokens: &[&str]) -> RunPlan {
    RunPlan::resolve(workspace, &TargetExpr::parse(tokens).unwrap()).unwrap()
}

fn names(plan: &RunPlan) -> Vec<&str> {
    plan.services()
        .iter()
        .map(|planned| planned.service.name.as_str())
        .collect()
}

#[test]
fn targets_parse_names_profiles_and_modifiers() {
    let expr = TargetExpr::parse(["web", "@fullstack", "+kg", "-api", "-@data"]).unwrap();
    assert_eq!(
        expr.ops(),
        [
            TargetOp::Include(TargetRef::Name("web".into())),
            TargetOp::Include(TargetRef::Profile("fullstack".into())),
            TargetOp::Include(TargetRef::Name("kg".into())),
            TargetOp::Exclude(TargetRef::Name("api".into())),
            TargetOp::Exclude(TargetRef::Profile("data".into())),
        ]
    );
}

#[test]
fn targets_split_on_commas() {
    assert_eq!(
        TargetExpr::parse(["web,api", " kg ,"]).unwrap(),
        TargetExpr::including(["web", "api", "kg"])
    );
}

#[test]
fn malformed_targets_are_rejected() {
    for raw in ["+", "-", "@", "-@", "Web", "+@Data"] {
        assert_eq!(
            TargetExpr::parse([raw]).unwrap_err().to_string(),
            format!("invalid target `{raw}`: expected `name`, `@profile`, `+name` or `-name`")
        );
    }
}

#[test]
fn needs_are_placed_before_the_services_that_need_them() {
    let plan = plan(&jaspy_like(), &["web", "kg"]);
    assert_eq!(names(&plan), ["web", "mongo", "neo4j", "knowledge-graph"]);
    let reasons: Vec<_> = plan
        .services()
        .iter()
        .map(|planned| planned.reason.clone())
        .collect();
    assert_eq!(
        reasons,
        [
            PlanReason::Requested,
            PlanReason::NeededBy(name("knowledge-graph")),
            PlanReason::NeededBy(name("knowledge-graph")),
            PlanReason::Requested,
        ]
    );
}

#[test]
fn profiles_expand_and_modifiers_apply_left_to_right() {
    let workspace = jaspy_like();
    assert_eq!(
        names(&plan(&workspace, &["@fullstack", "+kg", "-web"])),
        ["api", "mongo", "neo4j", "knowledge-graph"]
    );
    assert_eq!(
        names(&plan(&workspace, &["@fullstack", "-web", "+fe"])),
        ["api", "web"]
    );
}

#[test]
fn excluded_needs_are_skipped_and_reported() {
    let plan = plan(&jaspy_like(), &["kg", "-mongo"]);
    assert_eq!(names(&plan), ["neo4j", "knowledge-graph"]);
    assert_eq!(
        plan.skipped_needs(),
        [SkippedNeed {
            service: name("knowledge-graph"),
            need: name("mongo")
        }]
    );
}

#[test]
fn shared_needs_are_planned_once_and_explicit_requests_win() {
    let workspace = RepoWorkspace::from_parts(
        root(),
        [
            service("db"),
            service_needing("a", &["db"]),
            service_needing("b", &["db"]),
        ],
        [],
        [],
    )
    .unwrap();
    let plan = plan(&workspace, &["a", "b", "db"]);
    assert_eq!(names(&plan), ["db", "a", "b"]);
    assert_eq!(plan.services()[0].reason, PlanReason::Requested);
}

#[test]
fn empty_selections_are_rejected() {
    let workspace = jaspy_like();
    for tokens in [&[][..], &["web", "-web"][..], &["-web"][..]] {
        let error = RunPlan::resolve(&workspace, &TargetExpr::parse(tokens).unwrap()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "nothing selected: name at least one service, alias or @profile"
        );
    }
}

#[test]
fn unknown_targets_fail_resolution() {
    let error =
        RunPlan::resolve(&jaspy_like(), &TargetExpr::parse(["@nope"]).unwrap()).unwrap_err();
    assert_eq!(error.to_string(), "unknown profile `@nope`");
}

#[test]
fn port_conflicts_inside_a_plan_are_rejected() {
    let workspace = RepoWorkspace::from_parts(
        root(),
        [
            service("one").with_ports([port(8080)]),
            service("two").with_ports([port(8080)]),
        ],
        [],
        [],
    )
    .unwrap();
    assert!(RunPlan::resolve(&workspace, &TargetExpr::parse(["one"]).unwrap()).is_ok());
    let error =
        RunPlan::resolve(&workspace, &TargetExpr::parse(["one", "two"]).unwrap()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "port 8080 is used by both `one` and `two`"
    );
}

#[test]
fn requested_order_is_preserved() {
    let plan = plan(&jaspy_like(), &["api", "fe"]);
    let expected: Vec<ServiceName> = vec![name("api"), name("web")];
    let actual: Vec<ServiceName> = plan
        .services()
        .iter()
        .map(|planned| planned.service.name.clone())
        .collect();
    assert_eq!(actual, expected);
}
