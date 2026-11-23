use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_DEPTH: usize = 4;
const IGNORED_DIRS: [&str; 6] = [
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
];

pub fn directories(root: &Path) -> Vec<PathBuf> {
    let mut found = vec![root.to_path_buf()];
    descend(root, 0, &mut found);
    found
}

pub fn is_ignored(name: &OsStr) -> bool {
    name.to_str()
        .is_none_or(|name| name.starts_with('.') || IGNORED_DIRS.contains(&name))
}

pub fn crosses_ignored_dir(relative: &Path) -> bool {
    relative.components().any(|component| match component {
        Component::Normal(name) => is_ignored(name),
        _ => false,
    })
}

fn descend(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth == MAX_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| !is_ignored(&entry.file_name()))
        .map(|entry| entry.path())
        .collect();
    children.sort();
    for child in children {
        found.push(child.clone());
        descend(&child, depth + 1, found);
    }
}
