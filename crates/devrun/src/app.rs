use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow};
use devrun_domain::{RepoWorkspace, WorkspaceRepository};

use crate::adapters::{TomlWorkspaceStore, config_dir, repo_root};

pub struct App {
    root: PathBuf,
    store: TomlWorkspaceStore,
}

impl App {
    pub fn open(repo: Option<&Path>) -> anyhow::Result<Self> {
        let start = match repo {
            Some(path) => path.to_path_buf(),
            None => env::current_dir().context("cannot read the current directory")?,
        };
        Ok(Self {
            root: repo_root::locate(&start)?,
            store: TomlWorkspaceStore::new(config_dir::locate()?),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn repo_label(&self) -> String {
        self.root.file_name().map_or_else(
            || self.root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    pub fn store(&self) -> &TomlWorkspaceStore {
        &self.store
    }

    pub fn workspace(&self) -> anyhow::Result<RepoWorkspace> {
        self.store.load(&self.root)?.ok_or_else(|| {
            anyhow!(
                "no devrun config for {}; run `devrun init` to discover services",
                self.root.display()
            )
        })
    }
}
