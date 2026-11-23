use std::cell::RefCell;
use std::collections::VecDeque;
use std::io;
use std::path::{Path, PathBuf};

use devrun_domain::{
    AliasName, Candidate, CommandLine, Confidence, Port, RelativePath, RepoWorkspace, Runtime,
    Service, ServiceName, WorkspaceRepository,
};

use super::prompter::{CANCELLED, Choice, Prompter, Validator};
use super::wizard::{self, Outcome, Repo};
use crate::adapters::detectors::Discovery;

enum Answer {
    Defaults,
    Pick(&'static [&'static str]),
    Text(&'static str),
    Yes,
    No,
    Cancel,
}

#[derive(Default)]
struct Scripted {
    answers: VecDeque<Answer>,
    multiselects: Vec<(String, Vec<Choice>)>,
    inputs: Vec<(String, String)>,
    warnings: Vec<String>,
    errors: Vec<String>,
    outro: Option<String>,
    cancelled: Option<String>,
}

impl Scripted {
    fn new(answers: impl IntoIterator<Item = Answer>) -> Self {
        Self {
            answers: answers.into_iter().collect(),
            ..Self::default()
        }
    }

    fn next(&mut self) -> io::Result<Answer> {
        self.answers
            .pop_front()
            .ok_or_else(|| io::Error::other("script exhausted"))
    }

    fn choices(&self, prompt: &str) -> Vec<(String, bool)> {
        self.multiselects
            .iter()
            .find(|(asked, _)| asked == prompt)
            .map(|(_, choices)| {
                choices
                    .iter()
                    .map(|choice| (choice.label.clone(), choice.checked))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn interrupted() -> io::Error {
    io::ErrorKind::Interrupted.into()
}

fn unexpected() -> io::Error {
    io::Error::other("unexpected answer for this prompt")
}

impl Prompter for Scripted {
    fn intro(&mut self, _title: &str) -> io::Result<()> {
        Ok(())
    }

    fn start_spinner(&mut self, _message: &str) {}

    fn stop_spinner(&mut self, _message: &str) {}

    fn note(&mut self, _title: &str, _body: &str) -> io::Result<()> {
        Ok(())
    }

    fn warning(&mut self, message: &str) -> io::Result<()> {
        self.warnings.push(message.to_owned());
        Ok(())
    }

    fn error(&mut self, message: &str) -> io::Result<()> {
        self.errors.push(message.to_owned());
        Ok(())
    }

    fn multiselect(
        &mut self,
        prompt: &str,
        choices: &[Choice],
        _required: bool,
    ) -> io::Result<Vec<usize>> {
        self.multiselects
            .push((prompt.to_owned(), choices.to_vec()));
        let indices = |keep: &dyn Fn(&Choice) -> bool| {
            choices
                .iter()
                .enumerate()
                .filter(|(_, choice)| keep(choice))
                .map(|(index, _)| index)
                .collect()
        };
        match self.next()? {
            Answer::Defaults => Ok(indices(&|choice| choice.checked)),
            Answer::Pick(labels) => Ok(indices(&|choice| labels.contains(&choice.label.as_str()))),
            Answer::Cancel => Err(interrupted()),
            _ => Err(unexpected()),
        }
    }

    fn input(&mut self, prompt: &str, initial: &str, validate: Validator) -> io::Result<String> {
        self.inputs.push((prompt.to_owned(), initial.to_owned()));
        loop {
            match self.next()? {
                Answer::Defaults => return Ok(initial.to_owned()),
                Answer::Text(text) => match validate(text) {
                    Ok(()) => return Ok(text.to_owned()),
                    Err(error) => self.errors.push(error),
                },
                Answer::Cancel => return Err(interrupted()),
                _ => return Err(unexpected()),
            }
        }
    }

    fn confirm(&mut self, _prompt: &str, initial: bool) -> io::Result<bool> {
        match self.next()? {
            Answer::Defaults => Ok(initial),
            Answer::Yes => Ok(true),
            Answer::No => Ok(false),
            Answer::Cancel => Err(interrupted()),
            _ => Err(unexpected()),
        }
    }

    fn outro(&mut self, message: &str) -> io::Result<()> {
        self.outro = Some(message.to_owned());
        Ok(())
    }

    fn cancelled(&mut self, message: &str) -> io::Result<()> {
        self.cancelled = Some(message.to_owned());
        Ok(())
    }
}

#[derive(Default)]
struct MemoryStore {
    existing: Option<RepoWorkspace>,
    saved: RefCell<Vec<RepoWorkspace>>,
}

impl WorkspaceRepository for MemoryStore {
    type Error = anyhow::Error;

    fn load(&self, _root: &Path) -> anyhow::Result<Option<RepoWorkspace>> {
        Ok(self.existing.clone())
    }

    fn save(&self, workspace: &RepoWorkspace) -> anyhow::Result<()> {
        self.saved.borrow_mut().push(workspace.clone());
        Ok(())
    }
}

impl MemoryStore {
    fn only_save(&self) -> RepoWorkspace {
        let saved = self.saved.borrow();
        assert_eq!(saved.len(), 1, "expected exactly one save");
        saved[0].clone()
    }
}

fn root() -> PathBuf {
    PathBuf::from("/repo")
}

fn repo() -> Repo {
    Repo {
        root: root(),
        label: "repo".to_owned(),
        config_path: PathBuf::from("/config/repo.toml"),
    }
}

fn service(name: &str, ports: &[u16]) -> Service {
    Service::new(
        ServiceName::parse(name).unwrap(),
        Runtime::Node,
        RelativePath::parse(&format!("apps/{name}")).unwrap(),
        CommandLine::parse("npm run dev").unwrap(),
    )
    .with_ports(ports.iter().map(|port| Port::new(*port).unwrap()))
}

fn candidate(name: &str, confidence: Confidence, ports: &[u16]) -> Candidate {
    Candidate::new(service(name, ports), confidence, "test evidence")
}

fn high(names: &[&str]) -> Vec<Candidate> {
    names
        .iter()
        .map(|name| candidate(name, Confidence::High, &[]))
        .collect()
}

fn run_wizard(prompter: &mut Scripted, store: &MemoryStore, candidates: Vec<Candidate>) -> Outcome {
    wizard::run(prompter, store, &repo(), |_| {
        Ok(Discovery {
            candidates,
            ..Discovery::default()
        })
    })
    .unwrap()
}

fn service_names(workspace: &RepoWorkspace) -> Vec<String> {
    workspace
        .services()
        .map(|service| service.name.to_string())
        .collect()
}

fn alias_pairs(workspace: &RepoWorkspace) -> Vec<(String, String)> {
    workspace
        .aliases()
        .iter()
        .map(|(alias, target)| (alias.to_string(), target.to_string()))
        .collect()
}

fn ports_of(workspace: &RepoWorkspace, name: &str) -> Vec<u16> {
    workspace
        .service(&ServiceName::parse(name).unwrap())
        .unwrap()
        .ports
        .iter()
        .map(|port| port.get())
        .collect()
}

fn owned(pairs: &[(&str, bool)]) -> Vec<(String, bool)> {
    pairs
        .iter()
        .map(|(label, checked)| ((*label).to_owned(), *checked))
        .collect()
}

#[test]
fn low_confidence_candidates_start_unchecked_and_the_rest_are_preselected() {
    let mut prompter = Scripted::new([Answer::Defaults, Answer::Defaults, Answer::No, Answer::Yes]);
    let store = MemoryStore::default();
    let candidates = vec![
        candidate("web", Confidence::High, &[]),
        candidate("knowledge-graph", Confidence::Medium, &[]),
        candidate("data-feeder", Confidence::Low, &[]),
    ];
    assert_eq!(
        run_wizard(&mut prompter, &store, candidates),
        Outcome::Saved
    );
    assert_eq!(
        prompter.choices("Services to keep"),
        owned(&[
            ("data-feeder", false),
            ("knowledge-graph", true),
            ("web", true)
        ])
    );
    let saved = store.only_save();
    assert_eq!(service_names(&saved), ["knowledge-graph", "web"]);
    assert_eq!(
        alias_pairs(&saved),
        [("kg".to_owned(), "knowledge-graph".to_owned())]
    );
    assert_eq!(
        prompter.outro.as_deref(),
        Some("Next: devrun up knowledge-graph")
    );
}

#[test]
fn conflicting_ports_can_be_reassigned_or_kept() {
    let mut prompter = Scripted::new([
        Answer::Defaults,
        Answer::Defaults,
        Answer::Text("0"),
        Answer::Text("3001"),
        Answer::No,
        Answer::Yes,
    ]);
    let store = MemoryStore::default();
    let candidates = vec![
        candidate("api", Confidence::High, &[3000]),
        candidate("web", Confidence::High, &[3000]),
    ];
    assert_eq!(
        run_wizard(&mut prompter, &store, candidates),
        Outcome::Saved
    );
    assert_eq!(prompter.warnings, ["port 3000 is claimed by api, web"]);
    assert_eq!(
        prompter.inputs,
        [
            ("Port for api".to_owned(), "3000".to_owned()),
            ("Port for web".to_owned(), "3000".to_owned())
        ]
    );
    assert_eq!(
        prompter.errors,
        ["invalid port `0`: ports must be between 1 and 65535"]
    );
    let saved = store.only_save();
    assert_eq!(ports_of(&saved, "api"), [3000]);
    assert_eq!(ports_of(&saved, "web"), [3001]);
}

#[test]
fn alias_suggestions_skip_collisions_with_services_and_with_each_other() {
    let mut prompter = Scripted::new([Answer::Defaults, Answer::Defaults, Answer::No, Answer::Yes]);
    let store = MemoryStore::default();
    let candidates = high(&[
        "knowledge-graph",
        "kg",
        "data-feeder",
        "document-format",
        "jaspy-brief",
    ]);
    assert_eq!(
        run_wizard(&mut prompter, &store, candidates),
        Outcome::Saved
    );
    assert_eq!(
        prompter.choices("Aliases to create"),
        owned(&[("jb", true)])
    );
    assert_eq!(
        alias_pairs(&store.only_save()),
        [("jb".to_owned(), "jaspy-brief".to_owned())]
    );
}

#[test]
fn cancelling_at_any_prompt_or_declining_the_save_writes_nothing() {
    let scripts = [
        vec![Answer::Cancel],
        vec![Answer::Defaults, Answer::Cancel],
        vec![Answer::Defaults, Answer::Defaults, Answer::Cancel],
        vec![
            Answer::Defaults,
            Answer::Defaults,
            Answer::No,
            Answer::Cancel,
        ],
        vec![Answer::Defaults, Answer::Defaults, Answer::No, Answer::No],
    ];
    for script in scripts {
        let mut prompter = Scripted::new(script);
        let store = MemoryStore::default();
        let candidates = high(&["web", "knowledge-graph"]);
        assert_eq!(
            run_wizard(&mut prompter, &store, candidates),
            Outcome::Cancelled
        );
        assert!(store.saved.borrow().is_empty());
        assert_eq!(prompter.cancelled.as_deref(), Some(CANCELLED));
        assert!(
            prompter.answers.is_empty(),
            "every scripted prompt was asked"
        );
    }
}

#[test]
fn deselecting_a_referenced_service_keeps_it_and_shows_the_domain_error() {
    let mut existing = RepoWorkspace::new(root()).unwrap();
    for name in ["api", "docs", "web"] {
        existing.upsert_service(service(name, &[])).unwrap();
    }
    existing
        .set_alias(
            AliasName::parse("fe").unwrap(),
            ServiceName::parse("web").unwrap(),
        )
        .unwrap();
    let store = MemoryStore {
        existing: Some(existing),
        ..MemoryStore::default()
    };
    let mut prompter = Scripted::new([Answer::Pick(&["api"]), Answer::No, Answer::Yes]);
    let candidates = high(&["api", "docs", "web"]);
    assert_eq!(
        run_wizard(&mut prompter, &store, candidates),
        Outcome::Saved
    );
    assert_eq!(
        prompter.errors,
        ["kept web: service `web` is still used by alias `fe`"]
    );
    let saved = store.only_save();
    assert_eq!(service_names(&saved), ["api", "web"]);
    assert_eq!(alias_pairs(&saved), [("fe".to_owned(), "web".to_owned())]);
}

#[test]
fn rerun_prechecks_saved_services_and_offers_discovery_only_candidates_as_new() {
    let mut existing = RepoWorkspace::new(root()).unwrap();
    existing.upsert_service(service("web", &[])).unwrap();
    let store = MemoryStore {
        existing: Some(existing),
        ..MemoryStore::default()
    };
    let mut prompter = Scripted::new([Answer::Defaults, Answer::No, Answer::Yes]);
    let candidates = vec![
        candidate("api", Confidence::High, &[]),
        candidate("feeder", Confidence::Low, &[]),
        candidate("web", Confidence::Low, &[]),
    ];
    assert_eq!(
        run_wizard(&mut prompter, &store, candidates),
        Outcome::Saved
    );
    assert_eq!(
        prompter.choices("Services to keep"),
        owned(&[("api", true), ("feeder", false), ("web", true)])
    );
    let hints: Vec<bool> = prompter.multiselects[0]
        .1
        .iter()
        .map(|choice| choice.hint.starts_with("new"))
        .collect();
    assert_eq!(hints, [true, true, false]);
    assert_eq!(service_names(&store.only_save()), ["api", "web"]);
}

#[test]
fn profiles_are_created_with_names_validated_by_the_domain() {
    let mut prompter = Scripted::new([
        Answer::Defaults,
        Answer::Yes,
        Answer::Text("Full Stack"),
        Answer::Text("fullstack"),
        Answer::Pick(&["web", "api"]),
        Answer::No,
        Answer::Yes,
    ]);
    let store = MemoryStore::default();
    assert_eq!(
        run_wizard(&mut prompter, &store, high(&["web", "api"])),
        Outcome::Saved
    );
    assert_eq!(
        prompter.errors,
        [
            "invalid profile name `Full Stack`: use 1-64 lowercase letters, digits, `-`, `_` or `.`, starting with a letter or digit"
        ]
    );
    let saved = store.only_save();
    let profiles: Vec<(String, Vec<String>)> = saved
        .profiles()
        .iter()
        .map(|(name, members)| {
            (
                name.to_string(),
                members.iter().map(ToString::to_string).collect(),
            )
        })
        .collect();
    assert_eq!(
        profiles,
        [(
            "fullstack".to_owned(),
            vec!["api".to_owned(), "web".to_owned()]
        )]
    );
    assert_eq!(
        prompter.outro.as_deref(),
        Some("Next: devrun up @fullstack")
    );
}
