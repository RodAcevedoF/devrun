use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use devrun_domain::{Candidate, CommandLine, Confidence, RelativePath, Runtime, Service};
use glob::{MatchOptions, Pattern};
use serde::Deserialize;

use super::ports::ports_in;
use super::walk::directories;
use super::{Detector, Discovery};

const MANIFEST: &str = "package.json";
const WORKSPACE_MATCH: MatchOptions = MatchOptions {
    case_sensitive: true,
    require_literal_separator: true,
    require_literal_leading_dot: false,
};
const RUN_SCRIPTS: [(&str, Confidence); 3] = [
    ("dev", Confidence::High),
    ("start:dev", Confidence::Medium),
    ("start", Confidence::Medium),
];
const LOCKFILES: [(&str, &str); 5] = [
    ("pnpm-lock.yaml", "pnpm"),
    ("yarn.lock", "yarn"),
    ("bun.lockb", "bun"),
    ("bun.lock", "bun"),
    ("package-lock.json", "npm"),
];
const PACKAGE_MANAGERS: [&str; 4] = ["npm", "pnpm", "yarn", "bun"];

pub struct NodeWorkspaces;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackageJson {
    name: Option<String>,
    package_manager: Option<String>,
    workspaces: Option<Workspaces>,
    #[serde(default)]
    scripts: BTreeMap<String, String>,
    #[serde(default)]
    dependencies: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    dev_dependencies: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Workspaces {
    List(Vec<String>),
    Object {
        #[serde(default)]
        packages: Vec<String>,
    },
}

#[derive(Debug, Deserialize)]
struct PnpmWorkspace {
    #[serde(default)]
    packages: Vec<String>,
}

struct Package {
    dir: PathBuf,
    manifest: PackageJson,
}

#[derive(Default)]
struct WorkspaceGlobs {
    included: Vec<Pattern>,
    excluded: Vec<Pattern>,
}

impl Package {
    fn manifest_path(&self) -> PathBuf {
        self.dir.join(MANIFEST)
    }

    fn has_script(&self, script: &str) -> bool {
        self.manifest.scripts.contains_key(script)
    }

    fn dependency_names(&self) -> BTreeSet<&str> {
        self.manifest
            .dependencies
            .keys()
            .chain(self.manifest.dev_dependencies.keys())
            .map(String::as_str)
            .collect()
    }
}

struct Monorepo<'a> {
    root: &'a Path,
    manager: &'static str,
    uses_turbo: bool,
    packages: Vec<Package>,
}

impl Detector for NodeWorkspaces {
    fn detect(&self, root: &Path) -> anyhow::Result<Discovery> {
        let mut discovery = Discovery::default();
        let manifest_path = root.join(MANIFEST);
        if !manifest_path.is_file() {
            return Ok(discovery);
        }
        let Some(manifest) = load_package(root, &manifest_path, &mut discovery) else {
            return Ok(discovery);
        };
        let patterns = workspace_patterns(root, &manifest, &mut discovery);
        let monorepo = Monorepo {
            root,
            manager: package_manager(root, &manifest),
            uses_turbo: root.join("turbo.json").is_file(),
            packages: load_packages(root, &patterns, &mut discovery),
        };
        for package in &monorepo.packages {
            if let Some(candidate) = monorepo.candidate(package, &mut discovery) {
                discovery.candidates.push(candidate);
            }
        }
        Ok(discovery)
    }
}

impl Monorepo<'_> {
    fn candidate(&self, package: &Package, discovery: &mut Discovery) -> Option<Candidate> {
        let (script, confidence) = RUN_SCRIPTS
            .into_iter()
            .find(|(script, _)| package.has_script(script))?;
        let body = &package.manifest.scripts[script];
        let dir_name = package.dir.file_name()?.to_string_lossy();
        let name = discovery.service_name(self.root, &package.dir, &dir_name)?;
        let path = discovery.relative(self.root, &package.dir)?;
        let manifest_path = package.manifest_path();
        let command =
            discovery.command(self.root, &manifest_path, [self.manager, "run", script])?;
        let prepare = self
            .prepare_words(package, &path)
            .into_iter()
            .map(|words| discovery.command(self.root, &manifest_path, words))
            .collect::<Option<Vec<CommandLine>>>()?;
        let service = Service::new(name, Runtime::Node, path, command)
            .with_ports(ports_in(body))
            .with_prepare(prepare);
        Some(Candidate::new(
            service,
            confidence,
            format!("{MANIFEST} script \"{script}\": {body}"),
        ))
    }

    fn prepare_words(&self, package: &Package, path: &RelativePath) -> Vec<Vec<String>> {
        let built = self.built_dependencies(package);
        if built.is_empty() {
            Vec::new()
        } else if self.uses_turbo {
            let filter = package
                .manifest
                .name
                .clone()
                .unwrap_or_else(|| format!("./{path}"));
            vec![owned(&[
                "npx",
                "turbo",
                "run",
                "build",
                &format!("--filter={filter}^..."),
            ])]
        } else {
            built
                .iter()
                .map(|dependency| owned(&[self.manager, "run", "build", "--workspace", dependency]))
                .collect()
        }
    }

    fn built_dependencies<'p>(&'p self, package: &Package) -> Vec<&'p str> {
        let wanted = package.dependency_names();
        self.packages
            .iter()
            .filter(|other| other.has_script("build"))
            .filter_map(|other| other.manifest.name.as_deref())
            .filter(|name| wanted.contains(name) && package.manifest.name.as_deref() != Some(name))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

impl WorkspaceGlobs {
    fn parse(root: &Path, patterns: &[String], discovery: &mut Discovery) -> Self {
        let mut globs = Self::default();
        for raw in patterns {
            let (negated, body) = raw
                .strip_prefix('!')
                .map_or((false, raw.as_str()), |body| (true, body));
            let body = body.trim_start_matches("./").trim_end_matches('/');
            match Pattern::new(body) {
                Ok(pattern) => globs.add(negated, pattern, body),
                Err(error) => discovery.skip(
                    root,
                    &root.join(MANIFEST),
                    format!("workspace pattern `{raw}`: {error}"),
                ),
            }
        }
        globs
    }

    fn add(&mut self, negated: bool, pattern: Pattern, body: &str) {
        let target = if negated {
            &mut self.excluded
        } else {
            &mut self.included
        };
        target.push(pattern);
        if let Some(base) = body
            .strip_suffix("/**")
            .and_then(|base| Pattern::new(base).ok())
        {
            target.push(base);
        }
    }

    fn contains(&self, relative: &Path) -> bool {
        let matches = |patterns: &[Pattern]| {
            patterns
                .iter()
                .any(|pattern| pattern.matches_path_with(relative, WORKSPACE_MATCH))
        };
        matches(&self.included) && !matches(&self.excluded)
    }
}

fn owned(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

fn load_package(root: &Path, path: &Path, discovery: &mut Discovery) -> Option<PackageJson> {
    let text = discovery.read(root, path)?;
    serde_json::from_str(&text)
        .map_err(|error| discovery.skip(root, path, error))
        .ok()
}

fn workspace_patterns(
    root: &Path,
    manifest: &PackageJson,
    discovery: &mut Discovery,
) -> Vec<String> {
    let mut patterns = match &manifest.workspaces {
        Some(Workspaces::List(patterns) | Workspaces::Object { packages: patterns }) => {
            patterns.clone()
        }
        None => Vec::new(),
    };
    let pnpm_path = root.join("pnpm-workspace.yaml");
    if pnpm_path.is_file()
        && let Some(text) = discovery.read(root, &pnpm_path)
    {
        match serde_yaml_ng::from_str::<PnpmWorkspace>(&text) {
            Ok(pnpm) => patterns.extend(pnpm.packages),
            Err(error) => discovery.skip(root, &pnpm_path, error),
        }
    }
    patterns
}

fn package_manager(root: &Path, manifest: &PackageJson) -> &'static str {
    let declared = manifest
        .package_manager
        .as_deref()
        .and_then(|spec| spec.split('@').next())
        .and_then(|name| PACKAGE_MANAGERS.into_iter().find(|known| *known == name));
    let locked = || {
        LOCKFILES
            .into_iter()
            .find(|(lockfile, _)| root.join(lockfile).is_file())
            .map(|(_, manager)| manager)
    };
    declared.or_else(locked).unwrap_or("npm")
}

fn load_packages(root: &Path, patterns: &[String], discovery: &mut Discovery) -> Vec<Package> {
    let globs = WorkspaceGlobs::parse(root, patterns, discovery);
    directories(root)
        .into_iter()
        .filter(|dir| globs.contains(dir.strip_prefix(root).unwrap_or(dir)))
        .filter(|dir| dir.join(MANIFEST).is_file())
        .filter_map(|dir| {
            let manifest = load_package(root, &dir.join(MANIFEST), discovery)?;
            Some(Package { dir, manifest })
        })
        .collect()
}
