#![allow(clippy::unwrap_used)]

mod support;

use support::Sandbox;

#[test]
fn plan_orders_needs_first_and_explains_why_each_service_runs() {
    let sandbox = Sandbox::new();
    sandbox.write_jaspy_config();
    sandbox
        .devrun()
        .args(["plan", "fe", "kg"])
        .assert()
        .success()
        .stderr("")
        .stdout(
            "Plan: 4 services
  1. web              node     requested
  2. mongo            compose  needed by knowledge-graph
  3. neo4j            compose  needed by knowledge-graph
  4. knowledge-graph  python   requested
",
        );
}

#[test]
fn plan_accepts_profiles_with_add_and_remove_modifiers() {
    let sandbox = Sandbox::new();
    sandbox.write_jaspy_config();
    sandbox
        .devrun()
        .args(["plan", "@fullstack", "-web", "+kg", "-mongo"])
        .assert()
        .success()
        .stdout(
            "Plan: 3 services
  1. api              node     requested
  2. neo4j            compose  needed by knowledge-graph
  3. knowledge-graph  python   requested
",
        )
        .stderr("warning: knowledge-graph needs mongo, which was excluded\n");
}

#[test]
fn plan_reports_unknown_targets_with_a_suggestion() {
    let sandbox = Sandbox::new();
    sandbox.write_jaspy_config();
    sandbox
        .devrun()
        .args(["plan", "wbe"])
        .assert()
        .code(1)
        .stdout("")
        .stderr("error: unknown service or alias `wbe` (did you mean `web`?)\n");
}

#[test]
fn plan_with_only_exclusions_selects_nothing() {
    let sandbox = Sandbox::new();
    sandbox.write_jaspy_config();
    sandbox
        .devrun()
        .args(["plan", "-web"])
        .assert()
        .code(1)
        .stderr("error: nothing selected: name at least one service, alias or @profile\n");
}

#[test]
fn plan_requires_at_least_one_target() {
    let sandbox = Sandbox::new();
    sandbox.write_jaspy_config();
    sandbox.devrun().arg("plan").assert().code(2);
}
