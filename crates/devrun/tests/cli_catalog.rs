#![allow(clippy::unwrap_used)]

mod support;

use std::fs;

use predicates::prelude::*;
use support::Sandbox;
use tempfile::TempDir;

#[test]
fn config_path_lives_in_the_config_dir_and_is_keyed_by_repository() {
    let sandbox = Sandbox::new();
    let path = sandbox.config_path();
    let name = path.file_name().unwrap().to_str().unwrap();
    assert_eq!(path.parent().unwrap(), sandbox.config_dir().join("repos"));
    assert!(name.starts_with("jaspy-monorepo-"), "{name}");
    assert!(
        std::path::Path::new(name)
            .extension()
            .is_some_and(|extension| extension == "toml"),
        "{name}"
    );
    assert!(
        !sandbox.config_dir().exists(),
        "config path must not create files"
    );
}

#[test]
fn config_path_is_the_same_from_any_subdirectory() {
    let sandbox = Sandbox::new();
    let from_root = sandbox.config_path();
    sandbox
        .devrun_in(&sandbox.root().join("apps/web"))
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(format!("{}\n", from_root.display()));
}

#[test]
fn repo_flag_selects_the_repository_explicitly() {
    let sandbox = Sandbox::new();
    let elsewhere = TempDir::new().unwrap();
    sandbox
        .devrun_in(elsewhere.path())
        .args(["--repo", sandbox.root().to_str().unwrap(), "config", "path"])
        .assert()
        .success()
        .stdout(format!("{}\n", sandbox.config_path().display()));
}

#[test]
fn commands_outside_a_git_repository_fail_with_guidance() {
    let sandbox = Sandbox::new();
    let outside = TempDir::new().unwrap();
    let outside = fs::canonicalize(outside.path()).unwrap();
    sandbox
        .devrun_in(&outside)
        .arg("ls")
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!(
            "error: not inside a git repository (searched from {}); pass --repo <path>\n",
            outside.display()
        ));
}

#[test]
fn ls_without_config_points_to_init() {
    let sandbox = Sandbox::new();
    sandbox
        .devrun()
        .arg("ls")
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!(
            "error: no devrun config for {}; run `devrun init` to discover services\n",
            sandbox.root().display()
        ));
}

#[test]
fn ls_lists_services_aliases_and_profiles() {
    let sandbox = Sandbox::new();
    sandbox.write_jaspy_config();
    sandbox
        .devrun()
        .arg("ls")
        .assert()
        .success()
        .stderr("")
        .stdout(format!(
            "jaspy-monorepo  {}

Services
  api              node     apps/api              :3000
  knowledge-graph  python   apps/knowledge-graph  :8081  needs mongo, neo4j
  mongo            compose  apps/knowledge-graph
  neo4j            compose  apps/knowledge-graph
  web              node     apps/web              :5173

Aliases
  fe  -> web
  kg  -> knowledge-graph

Profiles
  @fullstack  web, api
",
            sandbox.root().display()
        ));
}

#[test]
fn ls_shows_empty_sections_explicitly() {
    let sandbox = Sandbox::new();
    sandbox.write_config(
        "schema_version = 1\nroot = \"{root}\"\n\n[services.web]\nruntime = \"node\"\npath = \"apps/web\"\ncommand = \"npm run dev\"\n",
    );
    sandbox
        .devrun()
        .arg("ls")
        .assert()
        .success()
        .stdout(predicate::str::ends_with(
            "Services\n  web  node  apps/web\n\nAliases\n  (none)\n\nProfiles\n  (none)\n",
        ));
}

#[test]
fn unsupported_schema_versions_are_rejected() {
    let sandbox = Sandbox::new();
    let path = sandbox.write_config("schema_version = 2\nroot = \"{root}\"\n");
    sandbox.devrun().arg("ls").assert().code(1).stderr(format!(
        "error: unsupported schema_version 2 in {} (expected 1)\n",
        path.display()
    ));
}

#[test]
fn configs_that_break_workspace_rules_are_rejected() {
    let sandbox = Sandbox::new();
    let path = sandbox.write_config(&format!(
        "{}\n",
        support::JASPY_CONFIG.replace("fe = \"web\"", "api = \"web\"")
    ));
    sandbox.devrun().arg("ls").assert().code(1).stderr(format!(
        "error: invalid config {}: alias `api` has the same name as a service\n",
        path.display()
    ));
}

#[test]
fn malformed_toml_is_reported_with_its_path() {
    let sandbox = Sandbox::new();
    let path = sandbox.write_config("schema_version = \n");
    sandbox
        .devrun()
        .arg("ls")
        .assert()
        .code(1)
        .stderr(predicate::str::starts_with(format!(
            "error: invalid config {}: ",
            path.display()
        )));
}

#[test]
fn config_for_another_repository_is_rejected() {
    let sandbox = Sandbox::new();
    let path = sandbox.write_config("schema_version = 1\nroot = \"/somewhere/else\"\n");
    sandbox.devrun().arg("ls").assert().code(1).stderr(format!(
        "error: config {} belongs to /somewhere/else, not {}\n",
        path.display(),
        sandbox.root().display()
    ));
}
