#![allow(dead_code, clippy::unwrap_used)]

use std::collections::BTreeMap;
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

    pub fn from_fixture(name: &str) -> Self {
        let sandbox = Self::new();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        copy_tree(&fixture, &sandbox.root);
        sandbox
    }

    pub fn write(&self, relative: &str, contents: &str) -> &Self {
        let path = self.root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
        self
    }

    pub fn snapshot(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        collect_files(&self.root, &self.root, &mut files);
        files
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

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn collect_files(root: &Path, dir: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        if path.is_dir() {
            files.insert(relative, Vec::new());
            collect_files(root, &path, files);
        } else {
            files.insert(relative, fs::read(&path).unwrap());
        }
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
