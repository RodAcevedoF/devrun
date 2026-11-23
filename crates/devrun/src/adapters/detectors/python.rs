use std::path::Path;

use devrun_domain::{Candidate, Confidence, Port, Runtime, Service};

use super::ports::ports_in;
use super::walk::directories;
use super::{Detector, Discovery};

const MANIFEST: &str = "pyproject.toml";
const DOCKERFILE: &str = "Dockerfile";
const ENTRY_FILES: [&str; 3] = ["api.py", "main.py", "app.py"];
const LAUNCH_INSTRUCTIONS: [&str; 2] = ["CMD", "ENTRYPOINT"];
const VIRTUALENV_NOTE: &str =
    "needs an activated virtualenv (.venv is added to PATH automatically at launch when present)";

pub struct PythonProjects;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Runner {
    Uv,
    Poetry,
    Bare,
}

impl Runner {
    fn detect(dir: &Path, manifest: &toml::Table) -> Self {
        if dir.join("uv.lock").is_file() {
            Self::Uv
        } else if dir.join("poetry.lock").is_file()
            || table_at(manifest, &["tool", "poetry"]).is_some()
        {
            Self::Poetry
        } else {
            Self::Bare
        }
    }

    fn words(self) -> &'static [&'static str] {
        match self {
            Self::Uv => &["uv", "run"],
            Self::Poetry => &["poetry", "run"],
            Self::Bare => &[],
        }
    }
}

struct Launch {
    words: Vec<String>,
    source: &'static str,
    ports: Vec<Port>,
    confidence: Confidence,
    evidence: String,
}

impl Detector for PythonProjects {
    fn detect(&self, root: &Path) -> anyhow::Result<Discovery> {
        let mut discovery = Discovery::default();
        for dir in directories(root) {
            if dir.join(MANIFEST).is_file()
                && let Some(candidate) = candidate(root, &dir, &mut discovery)
            {
                discovery.candidates.push(candidate);
            }
        }
        Ok(discovery)
    }
}

fn candidate(root: &Path, dir: &Path, discovery: &mut Discovery) -> Option<Candidate> {
    let manifest_path = dir.join(MANIFEST);
    let text = discovery.read(root, &manifest_path)?;
    let manifest: toml::Table = toml::from_str(&text)
        .map_err(|error| discovery.skip(root, &manifest_path, error))
        .ok()?;
    let runner = Runner::detect(dir, &manifest);
    let launch = dev_script(&manifest, runner)
        .or_else(|| uvicorn_image(root, dir, discovery))
        .or_else(|| entry_file(dir, runner))?;
    let dir_name = dir.file_name()?.to_string_lossy();
    let name = discovery.service_name(root, dir, &dir_name)?;
    let path = discovery.relative(root, dir)?;
    let words = runner
        .words()
        .iter()
        .map(|word| (*word).to_owned())
        .chain(launch.words);
    let command = discovery.command(root, &dir.join(launch.source), words)?;
    let service = Service::new(name, Runtime::Python, path, command).with_ports(launch.ports);
    Some(Candidate::new(service, launch.confidence, launch.evidence))
}

fn table_at<'a>(manifest: &'a toml::Table, keys: &[&str]) -> Option<&'a toml::Table> {
    keys.iter()
        .try_fold(manifest, |table, key| table.get(*key)?.as_table())
}

fn dev_script(manifest: &toml::Table, runner: Runner) -> Option<Launch> {
    let script = [
        &["tool", "poetry", "scripts"][..],
        &["project", "scripts"][..],
    ]
    .into_iter()
    .find_map(|keys| table_at(manifest, keys)?.get("dev"))?;
    let body = script
        .as_str()
        .map_or_else(|| script.to_string(), ToOwned::to_owned);
    let evidence = format!("{MANIFEST} script \"dev\": {body}");
    let (confidence, evidence) = match runner {
        Runner::Bare => (Confidence::Low, format!("{evidence}; {VIRTUALENV_NOTE}")),
        Runner::Uv | Runner::Poetry => (Confidence::High, evidence),
    };
    Some(Launch {
        words: vec!["dev".to_owned()],
        source: MANIFEST,
        ports: ports_in(&body),
        confidence,
        evidence,
    })
}

fn uvicorn_image(root: &Path, dir: &Path, discovery: &mut Discovery) -> Option<Launch> {
    let dockerfile = dir.join(DOCKERFILE);
    if !dockerfile.is_file() {
        return None;
    }
    let text = discovery.read(root, &dockerfile)?;
    text.lines().filter_map(uvicorn_launch).next_back()
}

fn uvicorn_launch(line: &str) -> Option<Launch> {
    let (instruction, arguments) = line.trim().split_once(char::is_whitespace)?;
    let instruction = LAUNCH_INSTRUCTIONS
        .into_iter()
        .find(|known| known.eq_ignore_ascii_case(instruction))?;
    let tokens = instruction_tokens(arguments.trim());
    let position = tokens
        .iter()
        .position(|token| token == "uvicorn" || token.ends_with("/uvicorn"))?;
    let app = tokens
        .get(position + 1)
        .filter(|token| !token.starts_with('-'))?;
    let shown = tokens.join(" ");
    let ports = ports_in(&shown);
    let mut words = vec!["uvicorn".to_owned(), app.clone(), "--reload".to_owned()];
    if let Some(port) = ports.first() {
        words.extend(["--port".to_owned(), port.to_string()]);
    }
    Some(Launch {
        words,
        source: DOCKERFILE,
        ports,
        confidence: Confidence::Medium,
        evidence: format!("{DOCKERFILE} {instruction}: {shown}"),
    })
}

fn instruction_tokens(arguments: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(arguments).unwrap_or_else(|_| {
        arguments
            .split_whitespace()
            .map(ToOwned::to_owned)
            .collect()
    })
}

fn entry_file(dir: &Path, runner: Runner) -> Option<Launch> {
    let file = ENTRY_FILES
        .into_iter()
        .find(|file| dir.join(file).is_file())?;
    let interpreter = if runner == Runner::Bare {
        "python3"
    } else {
        "python"
    };
    Some(Launch {
        words: vec![interpreter.to_owned(), file.to_owned()],
        source: file,
        ports: Vec::new(),
        confidence: Confidence::Low,
        evidence: format!("entry file {file}"),
    })
}
