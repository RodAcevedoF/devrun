use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};

pub fn locate(start: &Path) -> anyhow::Result<PathBuf> {
    let start = fs::canonicalize(start)
        .with_context(|| format!("repository path {} does not exist", start.display()))?;
    match start.ancestors().find(|dir| dir.join(".git").exists()) {
        Some(root) => Ok(root.to_path_buf()),
        None => bail!(
            "not inside a git repository (searched from {}); pass --repo <path>",
            start.display()
        ),
    }
}
