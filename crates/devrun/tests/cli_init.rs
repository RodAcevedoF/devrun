#![allow(clippy::unwrap_used)]

mod support;

use support::Sandbox;

const FRESH_LS: &str = "
Services
  api                      node     apps/api
  chatbot-api              node     apps/chatbot-api
  document-upload-manager  python   apps/document-upload-manager  :8000
  jaspy-brief              node     apps/jaspy-brief              :9090
  knowledge-graph          python   apps/knowledge-graph          :8080        needs mongo, neo4j
  mongo                    compose  apps/knowledge-graph          :27017
  neo4j                    compose  apps/knowledge-graph          :7474 :7687
  web                      node     apps/web

Aliases
  ca   -> chatbot-api
  dum  -> document-upload-manager
  jb   -> jaspy-brief
  kg   -> knowledge-graph

Profiles
  (none)
";

const RERUN_LS: &str = "
Services
  api                      node     apps/api                      :3000
  chatbot-api              node     apps/chatbot-api
  document-upload-manager  python   apps/document-upload-manager  :8000
  jaspy-brief              node     apps/jaspy-brief              :9090
  knowledge-graph          python   apps/knowledge-graph          :8081  needs mongo, neo4j
  mongo                    compose  apps/knowledge-graph
  neo4j                    compose  apps/knowledge-graph
  web                      node     apps/web                      :5173

Aliases
  ca   -> chatbot-api
  dum  -> document-upload-manager
  fe   -> web
  jb   -> jaspy-brief
  kg   -> knowledge-graph

Profiles
  @fullstack  web, api
";

fn assert_ls(sandbox: &Sandbox, body: &str) {
    sandbox
        .devrun()
        .arg("ls")
        .assert()
        .success()
        .stderr("")
        .stdout(format!(
            "jaspy-monorepo  {}\n{body}",
            sandbox.root().display()
        ));
}

#[test]
fn init_yes_saves_high_and_medium_candidates_with_suggested_aliases() {
    let sandbox = Sandbox::from_fixture("jaspy-like");
    let path = sandbox.config_path();
    sandbox
        .devrun()
        .args(["init", "--yes"])
        .assert()
        .success()
        .stderr("")
        .stdout(format!(
            "Added 8 services and 4 aliases\nSaved {}\nNext: devrun up api\n",
            path.display()
        ));
    assert_ls(&sandbox, FRESH_LS);
}

#[test]
fn init_yes_rerun_keeps_existing_entries_and_adds_only_new_candidates() {
    let sandbox = Sandbox::from_fixture("jaspy-like");
    let path = sandbox.write_jaspy_config();
    sandbox
        .devrun()
        .args(["init", "--yes"])
        .assert()
        .success()
        .stderr("")
        .stdout(format!(
            "Added 3 services and 3 aliases\nSaved {}\nNext: devrun up @fullstack\n",
            path.display()
        ));
    assert_ls(&sandbox, RERUN_LS);
}

#[test]
fn init_without_a_terminal_and_without_yes_fails_and_writes_nothing() {
    let sandbox = Sandbox::from_fixture("jaspy-like");
    let before = sandbox.snapshot();
    sandbox
        .devrun()
        .arg("init")
        .assert()
        .code(1)
        .stdout("")
        .stderr(
            "error: devrun init needs a terminal; pass --yes to accept the discovered defaults\n",
        );
    assert!(!sandbox.config_dir().exists());
    assert_eq!(sandbox.snapshot(), before);
}

#[test]
fn init_never_writes_inside_the_repository() {
    let sandbox = Sandbox::from_fixture("jaspy-like");
    let before = sandbox.snapshot();
    sandbox.devrun().args(["init", "--yes"]).assert().success();
    sandbox.devrun().args(["init", "--yes"]).assert().success();
    assert_eq!(sandbox.snapshot(), before);
}
