use std::path::Path;

use devrun_domain::{Candidate, CommandLine, Confidence, Port, Runtime, Service};

use super::ports::ports_in;
use super::walk::directories;
use super::{Detector, Discovery};

const MANIFEST: &str = "pyproject.toml";
const DOCKERFILE: &str = "Dockerfile";
const ENTRY_FILES: [&str; 3] = ["api.py", "main.py", "app.py"];
const LAUNCH_INSTRUCTIONS: [&str; 2] = ["CMD", "ENTRYPOINT"];

pub struct PythonProjects;

struct Launch {
    command: String,
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
    let launch = dev_script(&manifest)
        .or_else(|| uvicorn_image(root, dir, discovery))
        .or_else(|| entry_file(dir))?;
    let dir_name = dir.file_name()?.to_string_lossy();
    let name = discovery.service_name(root, dir, &dir_name)?;
    let path = discovery.relative(root, dir)?;
    let command = CommandLine::parse(&with_runner(runner(dir, &manifest), &launch.command)).ok()?;
    let service = Service::new(name, Runtime::Python, path, command).with_ports(launch.ports);
    Some(Candidate::new(service, launch.confidence, launch.evidence))
}

fn runner(dir: &Path, manifest: &toml::Table) -> &'static str {
    if dir.join("uv.lock").is_file() {
        "uv run"
    } else if dir.join("poetry.lock").is_file() || table_at(manifest, &["tool", "poetry"]).is_some()
    {
        "poetry run"
    } else {
        ""
    }
}

fn with_runner(runner: &str, command: &str) -> String {
    if runner.is_empty() {
        command.to_owned()
    } else {
        format!("{runner} {command}")
    }
}

fn table_at<'a>(manifest: &'a toml::Table, keys: &[&str]) -> Option<&'a toml::Table> {
    keys.iter()
        .try_fold(manifest, |table, key| table.get(*key)?.as_table())
}

fn dev_script(manifest: &toml::Table) -> Option<Launch> {
    let script = [
        &["tool", "poetry", "scripts"][..],
        &["project", "scripts"][..],
    ]
    .into_iter()
    .find_map(|keys| table_at(manifest, keys)?.get("dev"))?;
    let body = script
        .as_str()
        .map_or_else(|| script.to_string(), ToOwned::to_owned);
    Some(Launch {
        command: "dev".to_owned(),
        ports: ports_in(&body),
        confidence: Confidence::High,
        evidence: format!("{MANIFEST} script \"dev\": {body}"),
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
    let command = match ports.first() {
        Some(port) => format!("uvicorn {app} --reload --port {port}"),
        None => format!("uvicorn {app} --reload"),
    };
    Some(Launch {
        command,
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

fn entry_file(dir: &Path) -> Option<Launch> {
    let file = ENTRY_FILES
        .into_iter()
        .find(|file| dir.join(file).is_file())?;
    Some(Launch {
        command: format!("python {file}"),
        ports: Vec::new(),
        confidence: Confidence::Low,
        evidence: format!("entry file {file}"),
    })
}
