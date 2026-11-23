#![allow(dead_code, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use tempfile::TempDir;

pub struct Sandbox {
    _home: TempDir,
    root: PathBuf,
    config: PathBuf,
}

impl Sandbox {
    pub fn new() -> Self {
        let home = TempDir::new().unwrap();
        let base = fs::canonicalize(home.path()).unwrap();
        let root = base.join("jaspy-monorepo");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        let config = base.join("config");
        Self {
            _home: home,
            root,
            config,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    pub fn devrun(&self) -> Command {
        self.devrun_in(&self.root)
    }

    pub fn devrun_in(&self, dir: &Path) -> Command {
        let mut command = Command::cargo_bin("devrun").unwrap();
        command
            .current_dir(dir)
            .env("DEVRUN_CONFIG_DIR", &self.config)
            .env_remove("XDG_CONFIG_HOME")
            .env("NO_COLOR", "1");
        command
    }

    pub fn config_path(&self) -> PathBuf {
        let output = self.devrun().args(["config", "path"]).output().unwrap();
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    pub fn write_config(&self, body: &str) -> PathBuf {
        let path = self.config_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            body.replace("{root}", &self.root.display().to_string()),
        )
        .unwrap();
        path
    }

    pub fn write_jaspy_config(&self) -> PathBuf {
        self.write_config(JASPY_CONFIG)
    }
}

pub const JASPY_CONFIG: &str = r#"schema_version = 1
root = "{root}"

[services.api]
runtime = "node"
path = "apps/api"
command = "npm run dev"
ports = [3000]

[services.knowledge-graph]
runtime = "python"
path = "apps/knowledge-graph"
command = "poetry run uvicorn src.entrypoints.http.app:app --reload --port 8081"
ports = [8081]
needs = ["mongo", "neo4j"]

[services.mongo]
runtime = "compose"
path = "apps/knowledge-graph"
command = "docker compose up mongo"

[services.neo4j]
runtime = "compose"
path = "apps/knowledge-graph"
command = "docker compose up neo4j"

[services.web]
runtime = "node"
path = "apps/web"
command = "npm run dev"
ports = [5173]

[aliases]
fe = "web"
kg = "knowledge-graph"

[profiles]
fullstack = ["web", "api"]
"#;
