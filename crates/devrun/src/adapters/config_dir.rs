use std::env;
use std::path::PathBuf;

use anyhow::Context;

fn env_path(key: &str) -> Option<PathBuf> {
    env::var_os(key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

pub fn locate() -> anyhow::Result<PathBuf> {
    if let Some(dir) = env_path("DEVRUN_CONFIG_DIR") {
        return Ok(dir);
    }
    if let Some(base) = env_path("XDG_CONFIG_HOME") {
        return Ok(base.join("devrun"));
    }
    env_path("HOME")
        .map(|home| home.join(".config").join("devrun"))
        .context("cannot locate the config directory: set HOME or DEVRUN_CONFIG_DIR")
}
