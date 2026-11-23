#![allow(clippy::unwrap_used)]

mod support;

use std::fs;

use support::Sandbox;

fn configured() -> Sandbox {
    let sandbox = Sandbox::new();
    sandbox.write_jaspy_config();
    sandbox
}

fn assert_rejected(sandbox: &Sandbox, args: &[&str], stderr: &str) {
    let path = sandbox.config_path();
    let before = fs::read(&path).unwrap();
    sandbox
        .devrun()
        .args(args)
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!("{stderr}\n"));
    assert_eq!(
        fs::read(&path).unwrap(),
        before,
        "config must stay unchanged"
    );
}

fn assert_prints(sandbox: &Sandbox, args: &[&str], stdout: &str) {
    sandbox
        .devrun()
        .args(args)
        .assert()
        .success()
        .stderr("")
        .stdout(stdout.to_owned());
}

#[test]
fn alias_set_creates_an_alias_and_reports_it() {
    let sandbox = configured();
    assert_prints(
        &sandbox,
        &["alias", "set", "be", "api"],
        "alias be -> api\n",
    );
    assert_prints(
        &sandbox,
        &["alias", "ls"],
        "Aliases\n  be  -> api\n  fe  -> web\n  kg  -> knowledge-graph\n",
    );
}

#[test]
fn alias_set_replaces_an_existing_alias() {
    let sandbox = configured();
    assert_prints(
        &sandbox,
        &["alias", "set", "kg", "web"],
        "alias kg -> web\n",
    );
    assert_prints(
        &sandbox,
        &["alias", "ls"],
        "Aliases\n  fe  -> web\n  kg  -> web\n",
    );
}

#[test]
fn alias_set_resolves_an_alias_target_to_its_service() {
    let sandbox = configured();
    assert_prints(
        &sandbox,
        &["alias", "set", "graph", "kg"],
        "alias graph -> knowledge-graph\n",
    );
}

#[test]
fn alias_rm_removes_an_alias_and_reports_it() {
    let sandbox = configured();
    assert_prints(&sandbox, &["alias", "rm", "kg"], "removed alias kg\n");
    assert_prints(&sandbox, &["alias", "ls"], "Aliases\n  fe  -> web\n");
}

#[test]
fn alias_ls_shows_none_when_there_are_no_aliases() {
    let sandbox = configured();
    assert_prints(&sandbox, &["alias", "rm", "kg"], "removed alias kg\n");
    assert_prints(&sandbox, &["alias", "rm", "fe"], "removed alias fe\n");
    assert_prints(&sandbox, &["alias", "ls"], "Aliases\n  (none)\n");
}

#[test]
fn alias_shadowing_a_service_is_rejected_and_config_is_unchanged() {
    let sandbox = configured();
    assert_rejected(
        &sandbox,
        &["alias", "set", "api", "web"],
        "error: alias `api` has the same name as a service",
    );
}

#[test]
fn alias_to_an_unknown_service_is_rejected_with_a_suggestion() {
    let sandbox = configured();
    assert_rejected(
        &sandbox,
        &["alias", "set", "w", "wbe"],
        "error: unknown service or alias `wbe` (did you mean `web`?)",
    );
}

#[test]
fn alias_rm_of_an_unknown_alias_is_rejected() {
    let sandbox = configured();
    assert_rejected(
        &sandbox,
        &["alias", "rm", "zz"],
        "error: unknown alias `zz`",
    );
}

#[test]
fn alias_commands_require_a_config() {
    let sandbox = Sandbox::new();
    sandbox
        .devrun()
        .args(["alias", "ls"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(format!(
            "error: no devrun config for {}; run `devrun init` to discover services\n",
            sandbox.root().display()
        ));
    assert!(!sandbox.config_dir().exists());
}

#[test]
fn profile_set_resolves_aliases_to_service_names() {
    let sandbox = configured();
    assert_prints(
        &sandbox,
        &["profile", "set", "backend", "kg", "api", "knowledge-graph"],
        "profile @backend: knowledge-graph, api\n",
    );
    assert_prints(
        &sandbox,
        &["profile", "ls"],
        "Profiles\n  @backend    knowledge-graph, api\n  @fullstack  web, api\n",
    );
}

#[test]
fn profile_set_accepts_an_at_prefixed_name_and_replaces_members() {
    let sandbox = configured();
    assert_prints(
        &sandbox,
        &["profile", "set", "@fullstack", "fe", "api", "kg"],
        "profile @fullstack: web, api, knowledge-graph\n",
    );
}

#[test]
fn profile_with_an_unknown_member_is_rejected_with_a_suggestion() {
    let sandbox = configured();
    assert_rejected(
        &sandbox,
        &["profile", "set", "backend", "api", "wbe"],
        "error: unknown service or alias `wbe` (did you mean `web`?)",
    );
}

#[test]
fn profile_with_an_invalid_name_is_rejected() {
    let sandbox = configured();
    assert_rejected(
        &sandbox,
        &["profile", "set", "Back", "api"],
        "error: invalid profile name `Back`: use 1-64 lowercase letters, digits, `-`, `_` or `.`, starting with a letter or digit",
    );
}

#[test]
fn profile_set_without_members_and_without_a_terminal_fails() {
    let sandbox = configured();
    assert_rejected(
        &sandbox,
        &["profile", "set", "backend"],
        "error: no services given and no terminal for the picker; pass services explicitly",
    );
}

#[test]
fn profile_rm_removes_a_profile_and_reports_it() {
    let sandbox = configured();
    assert_prints(
        &sandbox,
        &["profile", "rm", "fullstack"],
        "removed profile @fullstack\n",
    );
    assert_prints(&sandbox, &["profile", "ls"], "Profiles\n  (none)\n");
}

#[test]
fn profile_rm_of_an_unknown_profile_is_rejected_with_a_suggestion() {
    let sandbox = configured();
    assert_rejected(
        &sandbox,
        &["profile", "rm", "fulstack"],
        "error: unknown profile `@fulstack` (did you mean `fullstack`?)",
    );
}

#[test]
fn profiles_saved_by_profile_set_show_up_in_ls() {
    let sandbox = configured();
    assert_prints(
        &sandbox,
        &["profile", "set", "graph", "kg"],
        "profile @graph: knowledge-graph\n",
    );
    sandbox
        .devrun()
        .arg("ls")
        .assert()
        .success()
        .stdout(predicates::str::ends_with(
            "Profiles\n  @fullstack  web, api\n  @graph      knowledge-graph\n",
        ));
}
