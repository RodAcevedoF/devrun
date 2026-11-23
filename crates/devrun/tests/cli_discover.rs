#![allow(clippy::unwrap_used)]

mod support;

use assert_cmd::assert::Assert;
use predicates::prelude::*;
use support::Sandbox;

const INIT_HINT: &str = "run `devrun init` to choose which services to keep";

const JASPY_ROWS: &str = r#"  api                      node     apps/api                                   high    npm run dev
    why: package.json script "dev": nest start --watch
    prepare: npx turbo run build --filter=jaspy-backend^...
  chatbot-api              node     apps/chatbot-api                           high    npm run dev
    why: package.json script "dev": tsc --watch & node --watch dist/index.js
  data-feeder              python   apps/data-feeder                           low     poetry run python main.py
    why: entry file main.py
  document-upload-manager  python   apps/document-upload-manager  :8000        high    poetry run dev
    why: pyproject.toml script "dev": uvicorn app.main:app --host 0.0.0.0 --port 8000 --reload
  jaspy-brief              node     apps/jaspy-brief              :9090        high    npm run dev
    why: package.json script "dev": next dev -p 9090
  knowledge-graph          python   apps/knowledge-graph          :8080        medium  poetry run uvicorn src.entrypoints.http.app:app --reload --port 8080
    why: Dockerfile CMD: python3.11 -m uvicorn src.entrypoints.http.app:app --host 0.0.0.0 --port 8080; needs compose services mongo, neo4j
  mongo                    compose  apps/knowledge-graph          :27017       high    docker compose -f docker-compose.yml up mongo
    why: docker-compose.yml service "mongo"
  neo4j                    compose  apps/knowledge-graph          :7474 :7687  high    docker compose -f docker-compose.yml up neo4j
    why: docker-compose.yml service "neo4j"
  research-engine          python   apps/research-engine                       low     poetry run python api.py
    why: entry file api.py
  web                      node     apps/web                                   high    npm run dev
    why: package.json script "dev": vite
"#;

fn discover(sandbox: &Sandbox) -> Assert {
    sandbox.devrun().arg("discover").assert()
}

fn new_only(count: &str, rows: &str) -> String {
    format!("Discovery: {count}\n\nNew\n{rows}\n{INIT_HINT}\n")
}

fn jaspy_sandbox() -> Sandbox {
    let sandbox = Sandbox::from_fixture("jaspy-like");
    sandbox
        .write(
            "node_modules/fake/package.json",
            r#"{ "name": "fake", "scripts": { "dev": "node fake.js" } }"#,
        )
        .write(
            "node_modules/fake/pyproject.toml",
            "[project.scripts]\ndev = \"fake:dev\"\n",
        );
    sandbox
}

fn node_workspace(sandbox: &Sandbox, packages: &[(&str, &str)]) {
    sandbox.write("package.json", r#"{ "workspaces": ["apps/*"] }"#);
    for (dir, manifest) in packages {
        sandbox.write(&format!("apps/{dir}/package.json"), manifest);
    }
}

fn plain_python(sandbox: &Sandbox, dir: &str, files: &[&str]) {
    sandbox.write(
        &format!("{dir}/pyproject.toml"),
        &format!("[project]\nname = \"{dir}\"\n"),
    );
    for file in files {
        sandbox.write(&format!("{dir}/{file}"), "");
    }
}

fn assert_warning_prefixes(output: &Assert, prefixes: &[&str]) {
    let stderr = String::from_utf8(output.get_output().stderr.clone()).unwrap();
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), prefixes.len(), "stderr: {stderr}");
    for (line, prefix) in lines.iter().zip(prefixes) {
        assert!(
            line.starts_with(prefix),
            "{line:?} should start with {prefix:?}"
        );
        assert!(
            line.len() > prefix.len(),
            "{line:?} should explain the skip"
        );
    }
}

const VITE_WEB: &str =
    "  web  node  apps/web  high  npm run dev\n    why: package.json script \"dev\": vite\n";

#[test]
fn help_lists_the_discover_command() {
    Sandbox::new()
        .devrun()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "discover  Scan the repository for services and compare them with the config",
        ));
}

#[test]
fn discover_without_config_lists_every_candidate_as_new() {
    let sandbox = jaspy_sandbox();
    discover(&sandbox)
        .success()
        .stderr("")
        .stdout(new_only("10 services", JASPY_ROWS));
}

#[test]
fn discover_with_config_shows_new_changed_and_missing_services() {
    let sandbox = jaspy_sandbox();
    sandbox.write_config(&format!(
        "{}{}",
        support::JASPY_CONFIG,
        r#"
[services.legacy]
runtime = "node"
path = "apps/legacy"
command = "npm run dev"

[services.tunnel]
runtime = "custom"
path = "."
command = "ngrok http 3000"
"#
    ));
    discover(&sandbox).success().stderr("").stdout(
        r#"Discovery: 10 services

New
  chatbot-api              node    apps/chatbot-api                     high  npm run dev
    why: package.json script "dev": tsc --watch & node --watch dist/index.js
  data-feeder              python  apps/data-feeder                     low   poetry run python main.py
    why: entry file main.py
  document-upload-manager  python  apps/document-upload-manager  :8000  high  poetry run dev
    why: pyproject.toml script "dev": uvicorn app.main:app --host 0.0.0.0 --port 8000 --reload
  jaspy-brief              node    apps/jaspy-brief              :9090  high  npm run dev
    why: package.json script "dev": next dev -p 9090
  research-engine          python  apps/research-engine                 low   poetry run python api.py
    why: entry file api.py

Changed
  api              high
    prepare: (none) -> npx turbo run build --filter=jaspy-backend^...
    why: package.json script "dev": nest start --watch
  knowledge-graph  medium
    command: poetry run uvicorn src.entrypoints.http.app:app --reload --port 8081 -> poetry run uvicorn src.entrypoints.http.app:app --reload --port 8080
    ports: :8081 -> :8080
    why: Dockerfile CMD: python3.11 -m uvicorn src.entrypoints.http.app:app --host 0.0.0.0 --port 8080; needs compose services mongo, neo4j
  mongo            high
    command: docker compose up mongo -> docker compose -f docker-compose.yml up mongo
    ports: (none) -> :27017
    why: docker-compose.yml service "mongo"
  neo4j            high
    command: docker compose up neo4j -> docker compose -f docker-compose.yml up neo4j
    ports: (none) -> :7474 :7687
    why: docker-compose.yml service "neo4j"

Missing
  legacy  node  apps/legacy  npm run dev
"#,
    );
}

#[test]
fn changed_services_show_runtime_and_path_differences() {
    let sandbox = Sandbox::new();
    node_workspace(&sandbox, &[("web", r#"{ "scripts": { "dev": "vite" } }"#)]);
    sandbox.write_config(
        "schema_version = 1\nroot = \"{root}\"\n\n[services.web]\nruntime = \"python\"\npath = \"web\"\ncommand = \"npm run dev\"\n",
    );
    discover(&sandbox).success().stderr("").stdout(
        r#"Discovery: 1 service

Changed
  web  high
    runtime: python -> node
    path: web -> apps/web
    why: package.json script "dev": vite
"#,
    );
}

#[test]
fn discover_reports_up_to_date_when_the_config_matches() {
    let sandbox = Sandbox::new();
    node_workspace(&sandbox, &[("web", r#"{ "scripts": { "dev": "vite" } }"#)]);
    sandbox.write_config(
        "schema_version = 1\nroot = \"{root}\"\n\n[services.web]\nruntime = \"node\"\npath = \"apps/web\"\ncommand = \"npm run dev\"\nports = [5173]\n",
    );
    discover(&sandbox)
        .success()
        .stderr("")
        .stdout("Discovery: 1 service\n\nUp to date\n");
}

#[test]
fn discover_never_writes_to_the_repository_or_the_config_dir() {
    let sandbox = jaspy_sandbox();
    let before = sandbox.snapshot();
    discover(&sandbox).success();
    assert_eq!(sandbox.snapshot(), before);
    assert!(!sandbox.config_dir().exists());
}

#[test]
fn node_scripts_prefer_dev_then_start_dev_then_start() {
    let sandbox = Sandbox::new();
    node_workspace(
        &sandbox,
        &[
            (
                "alpha",
                r#"{ "scripts": { "start": "node server.js", "dev": "node --watch server.js" } }"#,
            ),
            (
                "beta",
                r#"{ "scripts": { "start": "node b.js", "start:dev": "nodemon b.js" } }"#,
            ),
            ("gamma", r#"{ "scripts": { "start": "node g.js" } }"#),
            ("delta", r#"{ "scripts": { "build": "tsc" } }"#),
        ],
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "3 services",
        r#"  alpha  node  apps/alpha  high    npm run dev
    why: package.json script "dev": node --watch server.js
  beta   node  apps/beta   medium  npm run start:dev
    why: package.json script "start:dev": nodemon b.js
  gamma  node  apps/gamma  medium  npm run start
    why: package.json script "start": node g.js
"#,
    ));
}

#[test]
fn pnpm_workspace_yaml_lists_the_packages() {
    let sandbox = Sandbox::new();
    sandbox
        .write("package.json", r#"{ "name": "root" }"#)
        .write("pnpm-workspace.yaml", "packages:\n  - \"apps/*\"\n")
        .write("pnpm-lock.yaml", "lockfileVersion: '9.0'\n")
        .write(
            "apps/web/package.json",
            r#"{ "scripts": { "dev": "vite" } }"#,
        );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        &VITE_WEB.replace("npm run dev", "pnpm run dev"),
    ));
}

#[test]
fn workspaces_object_form_lists_the_packages() {
    let sandbox = Sandbox::new();
    sandbox
        .write(
            "package.json",
            r#"{ "workspaces": { "packages": ["apps/*"] } }"#,
        )
        .write(
            "apps/web/package.json",
            r#"{ "scripts": { "dev": "vite" } }"#,
        );
    discover(&sandbox)
        .success()
        .stderr("")
        .stdout(new_only("1 service", VITE_WEB));
}

#[test]
fn package_manager_field_wins_over_lockfiles() {
    let sandbox = Sandbox::new();
    sandbox
        .write(
            "package.json",
            r#"{ "packageManager": "yarn@4.1.0", "workspaces": ["apps/*"] }"#,
        )
        .write("package-lock.json", "{}")
        .write(
            "apps/web/package.json",
            r#"{ "scripts": { "dev": "vite" } }"#,
        );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        &VITE_WEB.replace("npm run dev", "yarn run dev"),
    ));
}

#[test]
fn lockfiles_pick_the_package_manager_and_npm_is_the_default() {
    for (lockfile, manager) in [
        (Some("bun.lockb"), "bun"),
        (Some("yarn.lock"), "yarn"),
        (Some("pnpm-lock.yaml"), "pnpm"),
        (None, "npm"),
    ] {
        let sandbox = Sandbox::new();
        node_workspace(&sandbox, &[("web", r#"{ "scripts": { "dev": "vite" } }"#)]);
        if let Some(lockfile) = lockfile {
            sandbox.write(lockfile, "");
        }
        discover(&sandbox).success().stderr("").stdout(new_only(
            "1 service",
            &VITE_WEB.replace("npm run dev", &format!("{manager} run dev")),
        ));
    }
}

#[test]
fn ports_are_parsed_from_script_flags_without_guessing_defaults() {
    let sandbox = Sandbox::new();
    node_workspace(
        &sandbox,
        &[
            ("a", r#"{ "scripts": { "dev": "next dev -p 9090" } }"#),
            ("b", r#"{ "scripts": { "dev": "vite --port 3001" } }"#),
            ("c", r#"{ "scripts": { "dev": "astro dev --port=3002" } }"#),
            (
                "d",
                r#"{ "scripts": { "dev": "PORT=3003 node server.js" } }"#,
            ),
            ("e", r#"{ "scripts": { "dev": "vite" } }"#),
        ],
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "5 services",
        r#"  a  node  apps/a  :9090  high  npm run dev
    why: package.json script "dev": next dev -p 9090
  b  node  apps/b  :3001  high  npm run dev
    why: package.json script "dev": vite --port 3001
  c  node  apps/c  :3002  high  npm run dev
    why: package.json script "dev": astro dev --port=3002
  d  node  apps/d  :3003  high  npm run dev
    why: package.json script "dev": PORT=3003 node server.js
  e  node  apps/e         high  npm run dev
    why: package.json script "dev": vite
"#,
    ));
}

fn api_with_workspace_dependencies(sandbox: &Sandbox) {
    sandbox
        .write(
            "package.json",
            r#"{ "workspaces": ["apps/*", "packages/*"] }"#,
        )
        .write(
            "apps/api/package.json",
            r#"{
  "name": "@repo/api",
  "scripts": { "dev": "node --watch src/main.js" },
  "dependencies": { "@repo/b": "*", "express": "^4.0.0" },
  "devDependencies": { "@repo/a": "*", "@repo/c": "*" }
}"#,
        )
        .write(
            "packages/a/package.json",
            r#"{ "name": "@repo/a", "scripts": { "build": "tsc" } }"#,
        )
        .write(
            "packages/b/package.json",
            r#"{ "name": "@repo/b", "scripts": { "build": "tsc" } }"#,
        )
        .write(
            "packages/c/package.json",
            r#"{ "name": "@repo/c", "scripts": { "lint": "eslint ." } }"#,
        );
}

#[test]
fn prepare_builds_each_workspace_dependency_without_turbo() {
    let sandbox = Sandbox::new();
    api_with_workspace_dependencies(&sandbox);
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        r#"  api  node  apps/api  high  npm run dev
    why: package.json script "dev": node --watch src/main.js
    prepare: npm run build --workspace @repo/a
    prepare: npm run build --workspace @repo/b
"#,
    ));
}

#[test]
fn prepare_uses_one_turbo_filter_when_turbo_json_exists() {
    let sandbox = Sandbox::new();
    api_with_workspace_dependencies(&sandbox);
    sandbox.write("turbo.json", "{}");
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        r#"  api  node  apps/api  high  npm run dev
    why: package.json script "dev": node --watch src/main.js
    prepare: npx turbo run build --filter=@repo/api^...
"#,
    ));
}

#[test]
fn libraries_and_node_modules_packages_are_skipped() {
    let sandbox = Sandbox::new();
    sandbox
        .write(
            "package.json",
            r#"{ "workspaces": ["apps/*", "packages/**"] }"#,
        )
        .write(
            "apps/web/package.json",
            r#"{ "scripts": { "dev": "vite" } }"#,
        )
        .write(
            "packages/ui/package.json",
            r#"{ "scripts": { "lint": "eslint ." } }"#,
        )
        .write(
            "packages/ui/node_modules/fake/package.json",
            r#"{ "scripts": { "dev": "node fake.js" } }"#,
        )
        .write(
            "packages/tools/cli/package.json",
            r#"{ "scripts": { "start": "node cli.js" } }"#,
        );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "2 services",
        r#"  cli  node  packages/tools/cli  medium  npm run start
    why: package.json script "start": node cli.js
  web  node  apps/web            high    npm run dev
    why: package.json script "dev": vite
"#,
    ));
}

#[test]
fn python_dev_script_wins_over_dockerfile_and_entry_files() {
    let sandbox = Sandbox::new();
    sandbox
        .write(
            "svc/pyproject.toml",
            "[project]\nname = \"svc\"\n\n[project.scripts]\ndev = \"svc.cli:dev\"\n",
        )
        .write("svc/uv.lock", "")
        .write(
            "svc/Dockerfile",
            "CMD [\"uvicorn\", \"svc.app:app\", \"--port\", \"8000\"]\n",
        )
        .write("svc/main.py", "");
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        "  svc  python  svc  high  uv run dev\n    why: pyproject.toml script \"dev\": svc.cli:dev\n",
    ));
}

#[test]
fn uvicorn_dockerfile_wins_over_entry_files() {
    let sandbox = Sandbox::new();
    plain_python(&sandbox, "api", &["main.py"]);
    sandbox.write(
        "api/Dockerfile",
        "FROM python:3.12\nCMD [\"python\", \"main.py\"]\nCMD uvicorn app.main:app --host 0.0.0.0 --port 9000\n",
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        "  api  python  api  :9000  medium  uvicorn app.main:app --reload --port 9000\n    why: Dockerfile CMD: uvicorn app.main:app --host 0.0.0.0 --port 9000\n",
    ));
}

#[test]
fn python_entry_files_are_tried_in_order() {
    let sandbox = Sandbox::new();
    plain_python(&sandbox, "one", &["main.py", "app.py"]);
    plain_python(&sandbox, "two", &["app.py"]);
    plain_python(&sandbox, "three", &["api.py", "main.py"]);
    plain_python(&sandbox, "four", &[]);
    discover(&sandbox).success().stderr("").stdout(new_only(
        "3 services",
        r"  one    python  one    low  python3 main.py
    why: entry file main.py
  three  python  three  low  python3 api.py
    why: entry file api.py
  two    python  two    low  python3 app.py
    why: entry file app.py
",
    ));
}

#[test]
fn python_runner_follows_lockfiles_and_poetry_config() {
    let sandbox = Sandbox::new();
    plain_python(&sandbox, "a", &["main.py", "uv.lock"]);
    plain_python(&sandbox, "b", &["main.py", "poetry.lock"]);
    plain_python(&sandbox, "d", &["main.py"]);
    sandbox
        .write("c/pyproject.toml", "[tool.poetry]\nname = \"c\"\n")
        .write("c/main.py", "");
    discover(&sandbox).success().stderr("").stdout(new_only(
        "4 services",
        r"  a  python  a  low  uv run python main.py
    why: entry file main.py
  b  python  b  low  poetry run python main.py
    why: entry file main.py
  c  python  c  low  poetry run python main.py
    why: entry file main.py
  d  python  d  low  python3 main.py
    why: entry file main.py
",
    ));
}

#[test]
fn scans_skip_ignored_hidden_and_deep_directories() {
    let sandbox = Sandbox::new();
    for dir in [
        "node_modules/x",
        ".venv/x",
        "venv/x",
        "__pycache__/x",
        "dist/x",
        "build/x",
        ".hidden/x",
        "a/b/c/d/e",
        "a/b/c/d",
    ] {
        plain_python(&sandbox, dir, &["main.py"]);
    }
    sandbox
        .write(
            "node_modules/x/compose.yml",
            "services:\n  db:\n    image: postgres\n",
        )
        .write(
            "build/compose.yml",
            "services:\n  db:\n    image: postgres\n",
        );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        "  d  python  a/b/c/d  low  python3 main.py\n    why: entry file main.py\n",
    ));
}

#[test]
fn compose_ports_use_the_host_side_including_long_syntax() {
    let sandbox = Sandbox::new();
    sandbox.write(
        "infra/compose.yaml",
        r#"services:
  cache:
    image: redis
    ports:
      - target: 6379
        published: 6380
      - target: 6390
        published: "6391"
  proxy:
    image: nginx
    ports:
      - "127.0.0.1:8443:443"
      - "8080:80/tcp"
      - "9000"
  worker:
    image: worker
"#,
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "3 services",
        r#"  cache   compose  infra  :6380 :6391  high  docker compose -f compose.yaml up cache
    why: compose.yaml service "cache"
  proxy   compose  infra  :8443 :8080  high  docker compose -f compose.yaml up proxy
    why: compose.yaml service "proxy"
  worker  compose  infra               high  docker compose -f compose.yaml up worker
    why: compose.yaml service "worker"
"#,
    ));
}

#[test]
fn compose_services_become_needs_of_apps_in_the_same_directory() {
    let sandbox = Sandbox::new();
    node_workspace(
        &sandbox,
        &[
            ("api", r#"{ "scripts": { "dev": "node server.js" } }"#),
            ("web", r#"{ "scripts": { "dev": "vite" } }"#),
        ],
    );
    sandbox.write(
        "apps/api/docker-compose.yml",
        "services:\n  db:\n    image: postgres\n    ports:\n      - \"5432:5432\"\n  cache:\n    image: redis\n",
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "4 services",
        r#"  api    node     apps/api         high  npm run dev
    why: package.json script "dev": node server.js; needs compose services cache, db
  cache  compose  apps/api         high  docker compose -f docker-compose.yml up cache
    why: docker-compose.yml service "cache"
  db     compose  apps/api  :5432  high  docker compose -f docker-compose.yml up db
    why: docker-compose.yml service "db"
  web    node     apps/web         high  npm run dev
    why: package.json script "dev": vite
"#,
    ));
}

#[test]
fn colliding_names_are_prefixed_with_their_directory() {
    let sandbox = Sandbox::new();
    plain_python(&sandbox, "apps/knowledge-graph", &["main.py"]);
    sandbox
        .write(
            "apps/knowledge-graph/docker-compose.yml",
            "services:\n  mongo:\n    image: mongo\n    ports:\n      - \"27017:27017\"\n",
        )
        .write(
            "apps/research-engine/compose.yml",
            "services:\n  mongo:\n    image: mongo\n    ports:\n      - \"27018:27017\"\n  redis:\n    image: redis\n",
        );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "4 services",
        r#"  knowledge-graph        python   apps/knowledge-graph          low   python3 main.py
    why: entry file main.py; needs compose services knowledge-graph-mongo
  knowledge-graph-mongo  compose  apps/knowledge-graph  :27017  high  docker compose -f docker-compose.yml up mongo
    why: docker-compose.yml service "mongo"
  redis                  compose  apps/research-engine          high  docker compose -f compose.yml up redis
    why: compose.yml service "redis"
  research-engine-mongo  compose  apps/research-engine  :27018  high  docker compose -f compose.yml up mongo
    why: compose.yml service "mongo"
"#,
    ));
}

#[test]
fn names_still_colliding_after_the_prefix_get_runtime_then_position_suffixes() {
    let sandbox = Sandbox::new();
    node_workspace(&sandbox, &[("api", r#"{ "scripts": { "dev": "vite" } }"#)]);
    plain_python(&sandbox, "apps/api", &["main.py"]);
    sandbox
        .write(
            "infra/compose.yml",
            "services:\n  db:\n    image: postgres\n",
        )
        .write(
            "infra/docker-compose.yml",
            "services:\n  db:\n    image: postgres\n",
        );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "4 services",
        r#"  apps-api-node       node     apps/api  high  npm run dev
    why: package.json script "dev": vite
  apps-api-python     python   apps/api  low   python3 main.py
    why: entry file main.py
  infra-db-compose    compose  infra     high  docker compose -f compose.yml up db
    why: compose.yml service "db"
  infra-db-compose-2  compose  infra     high  docker compose -f docker-compose.yml up db
    why: docker-compose.yml service "db"
"#,
    ));
}

#[test]
fn malformed_manifests_are_skipped_with_a_warning() {
    let sandbox = Sandbox::new();
    node_workspace(
        &sandbox,
        &[
            ("web", r#"{ "scripts": { "dev": "vite" } }"#),
            ("broken", r#"{ "scripts": "#),
        ],
    );
    sandbox
        .write("svc/pyproject.toml", "[project\nname = 1\n")
        .write("svc/main.py", "")
        .write("infra/compose.yml", "services: [unclosed\n");
    let output = discover(&sandbox)
        .success()
        .stdout(new_only("1 service", VITE_WEB));
    assert_warning_prefixes(
        &output,
        &[
            "warning: skipped apps/broken/package.json: ",
            "warning: skipped svc/pyproject.toml: ",
            "warning: skipped infra/compose.yml: ",
        ],
    );
}

#[test]
fn malformed_root_package_json_is_skipped_with_a_warning() {
    let sandbox = Sandbox::new();
    sandbox.write("package.json", "{ \"workspaces\": ");
    plain_python(&sandbox, "svc", &["main.py"]);
    let output = discover(&sandbox).success().stdout(new_only(
        "1 service",
        "  svc  python  svc  low  python3 main.py\n    why: entry file main.py\n",
    ));
    assert_warning_prefixes(&output, &["warning: skipped package.json: "]);
}

#[test]
fn shared_ports_are_reported_as_warnings() {
    let sandbox = Sandbox::new();
    node_workspace(
        &sandbox,
        &[
            ("alpha", r#"{ "scripts": { "dev": "vite --port 8080" } }"#),
            ("beta", r#"{ "scripts": { "dev": "next dev -p 8080" } }"#),
        ],
    );
    sandbox.write(
        "proxy/compose.yml",
        "services:\n  edge:\n    image: nginx\n    ports:\n      - \"8080:80\"\n",
    );
    discover(&sandbox)
        .success()
        .stderr("warning: port 8080 is claimed by alpha, beta, edge\n")
        .stdout(new_only(
            "3 services",
            r#"  alpha  node     apps/alpha  :8080  high  npm run dev
    why: package.json script "dev": vite --port 8080
  beta   node     apps/beta   :8080  high  npm run dev
    why: package.json script "dev": next dev -p 8080
  edge   compose  proxy       :8080  high  docker compose -f compose.yml up edge
    why: compose.yml service "edge"
"#,
        ));
}

#[test]
fn compose_service_keys_are_shell_quoted() {
    let sandbox = Sandbox::new();
    sandbox.write(
        "infra/compose.yml",
        "services:\n  \"db; touch pwned\":\n    image: postgres\n  \"it's\":\n    image: redis\n",
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "2 services",
        r#"  db-touch-pwned  compose  infra  high  docker compose -f compose.yml up 'db; touch pwned'
    why: compose.yml service "db; touch pwned"
  it-s            compose  infra  high  docker compose -f compose.yml up 'it'\''s'
    why: compose.yml service "it's"
"#,
    ));
}

#[test]
fn dockerfile_uvicorn_app_tokens_are_shell_quoted() {
    let sandbox = Sandbox::new();
    plain_python(&sandbox, "api", &[]);
    sandbox.write(
        "api/Dockerfile",
        "CMD [\"uvicorn\", \"$(touch pwned):app\", \"--port\", \"9000\"]\n",
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        "  api  python  api  :9000  medium  uvicorn '$(touch pwned):app' --reload --port 9000\n    why: Dockerfile CMD: uvicorn $(touch pwned):app --port 9000\n",
    ));
}

fn api_with_unsafe_package_names(sandbox: &Sandbox) {
    sandbox
        .write(
            "package.json",
            r#"{ "workspaces": ["apps/*", "packages/*"] }"#,
        )
        .write(
            "apps/api/package.json",
            r#"{
  "name": "@repo/api $(id)",
  "scripts": { "dev": "node main.js" },
  "dependencies": { "@repo/a b": "*" }
}"#,
        )
        .write(
            "packages/a/package.json",
            r#"{ "name": "@repo/a b", "scripts": { "build": "tsc" } }"#,
        );
}

const UNSAFE_API_ROW: &str = "  api  node  apps/api  high  npm run dev\n    why: package.json script \"dev\": node main.js\n";

#[test]
fn package_names_in_turbo_filters_are_shell_quoted() {
    let sandbox = Sandbox::new();
    api_with_unsafe_package_names(&sandbox);
    sandbox.write("turbo.json", "{}");
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        &format!(
            "{UNSAFE_API_ROW}    prepare: npx turbo run build '--filter=@repo/api $(id)^...'\n"
        ),
    ));
}

#[test]
fn package_names_in_workspace_builds_are_shell_quoted() {
    let sandbox = Sandbox::new();
    api_with_unsafe_package_names(&sandbox);
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        &format!("{UNSAFE_API_ROW}    prepare: npm run build --workspace '@repo/a b'\n"),
    ));
}

#[test]
fn tokens_with_newlines_or_nul_skip_the_candidate_with_a_warning() {
    let sandbox = Sandbox::new();
    plain_python(&sandbox, "api", &["main.py"]);
    sandbox
        .write("api/Dockerfile", "CMD [\"uvicorn\", \"app\\u0000:app\"]\n")
        .write(
            "infra/compose.yml",
            "services:\n  \"a\\nb\":\n    image: x\n  ok:\n    image: y\n",
        );
    discover(&sandbox)
        .success()
        .stderr(
            "warning: skipped api/Dockerfile: cannot quote \"app\\0:app\": contains a newline or NUL character
warning: skipped infra/compose.yml: cannot quote \"a\\nb\": contains a newline or NUL character
",
        )
        .stdout(new_only(
            "1 service",
            "  ok  compose  infra  high  docker compose -f compose.yml up ok\n    why: compose.yml service \"ok\"\n",
        ));
}

#[test]
fn changed_services_show_port_needs_and_prepare_differences() {
    let sandbox = Sandbox::new();
    sandbox
        .write(
            "package.json",
            r#"{ "workspaces": ["apps/*", "packages/*"] }"#,
        )
        .write(
            "apps/api/package.json",
            r#"{
  "name": "@repo/api",
  "scripts": { "dev": "node --watch main.js --port 3001" },
  "dependencies": { "@repo/a": "*" }
}"#,
        )
        .write(
            "packages/a/package.json",
            r#"{ "name": "@repo/a", "scripts": { "build": "tsc" } }"#,
        )
        .write(
            "apps/api/docker-compose.yml",
            "services:\n  db:\n    image: postgres\n",
        );
    sandbox.write_config(
        r#"schema_version = 1
root = "{root}"

[services.api]
runtime = "node"
path = "apps/api"
command = "npm run dev"
ports = [3000]
needs = ["cache"]
prepare = ["npm run build"]

[services.cache]
runtime = "custom"
path = "."
command = "redis-server"

[services.db]
runtime = "compose"
path = "apps/api"
command = "docker compose -f docker-compose.yml up db"
"#,
    );
    discover(&sandbox).success().stderr("").stdout(
        r#"Discovery: 2 services

Changed
  api  high
    ports: :3000 -> :3001
    needs: cache -> db
    prepare: npm run build -> npm run build --workspace @repo/a
    why: package.json script "dev": node --watch main.js --port 3001; needs compose services db
"#,
    );
}

#[test]
fn runner_less_dev_scripts_have_low_confidence_and_need_a_virtualenv() {
    let sandbox = Sandbox::new();
    sandbox.write(
        "svc/pyproject.toml",
        "[project]\nname = \"svc\"\n\n[project.scripts]\ndev = \"svc.cli:dev\"\n",
    );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "1 service",
        "  svc  python  svc  low  dev\n    why: pyproject.toml script \"dev\": svc.cli:dev; needs an activated virtualenv (.venv is added to PATH automatically at launch when present)\n",
    ));
}

#[cfg(unix)]
#[test]
fn recursive_workspace_patterns_never_descend_into_ignored_directories() {
    let sandbox = Sandbox::new();
    sandbox
        .write("package.json", r#"{ "workspaces": ["packages/**"] }"#)
        .write(
            "packages/ui/package.json",
            r#"{ "scripts": { "dev": "vite" } }"#,
        )
        .write(
            "packages/ui/node_modules/fake/package.json",
            r#"{ "scripts": { "dev": "node fake.js" } }"#,
        );
    let node_modules = sandbox.root().join("packages/ui/node_modules");
    for link in ["a", "b", "c", "d"] {
        std::os::unix::fs::symlink(".", node_modules.join(link)).unwrap();
    }
    sandbox
        .devrun()
        .arg("discover")
        .timeout(std::time::Duration::from_secs(20))
        .assert()
        .success()
        .stderr("")
        .stdout(new_only(
            "1 service",
            "  ui  node  packages/ui  high  npm run dev\n    why: package.json script \"dev\": vite\n",
        ));
}

#[test]
fn workspace_patterns_support_exact_paths_and_negations() {
    let sandbox = Sandbox::new();
    sandbox
        .write(
            "package.json",
            r#"{ "workspaces": ["apps/*", "!apps/legacy", "packages/**", "!packages/internal/**", "tools/runner"] }"#,
        )
        .write(
            "apps/web/package.json",
            r#"{ "scripts": { "dev": "vite" } }"#,
        )
        .write(
            "apps/legacy/package.json",
            r#"{ "scripts": { "dev": "node legacy.js" } }"#,
        )
        .write(
            "packages/cli/package.json",
            r#"{ "scripts": { "start": "node cli.js" } }"#,
        )
        .write(
            "packages/internal/tool/package.json",
            r#"{ "scripts": { "start": "node tool.js" } }"#,
        )
        .write(
            "tools/runner/package.json",
            r#"{ "scripts": { "start": "node run.js" } }"#,
        )
        .write(
            "tools/other/package.json",
            r#"{ "scripts": { "start": "node other.js" } }"#,
        );
    discover(&sandbox).success().stderr("").stdout(new_only(
        "3 services",
        r#"  cli     node  packages/cli  medium  npm run start
    why: package.json script "start": node cli.js
  runner  node  tools/runner  medium  npm run start
    why: package.json script "start": node run.js
  web     node  apps/web      high    npm run dev
    why: package.json script "dev": vite
"#,
    ));
}
