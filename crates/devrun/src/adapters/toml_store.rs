use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process;

use anyhow::{Context, bail};
use devrun_domain::{RepoWorkspace, ServiceName, WorkspaceRepository};

use super::workspace_file::{SCHEMA_VERSION, WorkspaceFile};

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0100_0000_01b3;

pub struct TomlWorkspaceStore {
    dir: PathBuf,
}

impl TomlWorkspaceStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn path_for(&self, root: &Path) -> PathBuf {
        let label = root
            .file_name()
            .and_then(|name| ServiceName::slug(&name.to_string_lossy()).ok())
            .map_or_else(|| "repo".to_owned(), |slug| slug.to_string());
        let fingerprint = fnv1a(root.as_os_str().as_encoded_bytes());
        self.dir
            .join("repos")
            .join(format!("{label}-{fingerprint:016x}.toml"))
    }
}

impl WorkspaceRepository for TomlWorkspaceStore {
    type Error = anyhow::Error;

    fn load(&self, root: &Path) -> anyhow::Result<Option<RepoWorkspace>> {
        let path = self.path_for(root);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot read {}", path.display()));
            }
        };
        let invalid = || format!("invalid config {}", path.display());
        let table: toml::Table = toml::from_str(&text).with_context(invalid)?;
        match table
            .get("schema_version")
            .and_then(toml::Value::as_integer)
        {
            Some(SCHEMA_VERSION) => {}
            Some(other) => bail!(
                "unsupported schema_version {other} in {} (expected {SCHEMA_VERSION})",
                path.display()
            ),
            None => bail!("{}: missing schema_version", invalid()),
        }
        let file: WorkspaceFile = table.try_into().with_context(invalid)?;
        if file.root != root {
            bail!(
                "config {} belongs to {}, not {}",
                path.display(),
                file.root.display(),
                root.display()
            );
        }
        file.into_domain().map(Some).with_context(invalid)
    }

    fn save(&self, workspace: &RepoWorkspace) -> anyhow::Result<()> {
        let path = self.path_for(workspace.root());
        let parent = path.parent().unwrap_or(&self.dir);
        fs::create_dir_all(parent)
            .with_context(|| format!("cannot create {}", parent.display()))?;
        let text = toml::to_string_pretty(&WorkspaceFile::from_domain(workspace))
            .context("cannot serialize the workspace")?;
        let staging = path.with_extension(format!("toml.{}.tmp", process::id()));
        fs::write(&staging, text).with_context(|| format!("cannot write {}", staging.display()))?;
        fs::rename(&staging, &path).with_context(|| format!("cannot replace {}", path.display()))
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}
