use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use devrun_domain::{RunPlan, Runtime, Service, ServiceName};
use devrun_supervisor::{LaunchSpec, ServiceId};

const PYTHON_RUNNERS: [&str; 2] = ["poetry run ", "uv run "];

pub fn launch_specs(plan: &RunPlan, root: &Path) -> Vec<LaunchSpec> {
    let planned: BTreeSet<&ServiceName> = plan
        .services()
        .iter()
        .map(|planned| &planned.service.name)
        .collect();
    plan.services()
        .iter()
        .map(|entry| launch_spec(&entry.service, root, &planned))
        .collect()
}

fn launch_spec(service: &Service, root: &Path, planned: &BTreeSet<&ServiceName>) -> LaunchSpec {
    let cwd = service.path.within(root);
    let interpreter = Interpreter::for_service(service.runtime, &cwd);
    LaunchSpec {
        id: service_id(&service.name),
        command: interpreter.adapt(service.command.as_str()),
        env: service
            .env
            .iter()
            .map(|(key, value)| (key.as_str().to_owned(), value.clone()))
            .collect(),
        path_prefix: interpreter.path_prefix(),
        prepare: service
            .prepare
            .iter()
            .map(|step| interpreter.adapt(step.as_str()))
            .collect(),
        ports: service.ports.iter().map(|port| port.get()).collect(),
        needs: service
            .needs
            .iter()
            .filter(|need| planned.contains(need))
            .map(service_id)
            .collect(),
        cwd,
    }
}

pub fn service_id(name: &ServiceName) -> ServiceId {
    ServiceId::new(name.as_str())
}

enum Interpreter {
    Inherited,
    Venv(PathBuf),
}

impl Interpreter {
    fn for_service(runtime: Runtime, cwd: &Path) -> Self {
        let bin = cwd.join(".venv").join("bin");
        if runtime == Runtime::Python && bin.is_dir() {
            Self::Venv(bin)
        } else {
            Self::Inherited
        }
    }

    fn adapt(&self, command: &str) -> String {
        match self {
            Self::Inherited => command.to_owned(),
            Self::Venv(_) => without_runner(command).to_owned(),
        }
    }

    fn path_prefix(&self) -> Vec<PathBuf> {
        match self {
            Self::Inherited => Vec::new(),
            Self::Venv(bin) => vec![bin.clone()],
        }
    }
}

fn without_runner(command: &str) -> &str {
    PYTHON_RUNNERS
        .iter()
        .find_map(|runner| command.strip_prefix(runner))
        .map_or(command, str::trim_start)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;

    use devrun_domain::{
        CommandLine, EnvKey, Port, RelativePath, RepoWorkspace, Runtime, Service, ServiceName,
        TargetExpr,
    };
    use devrun_supervisor::ServiceId;

    use super::*;

    fn name(raw: &str) -> ServiceName {
        ServiceName::parse(raw).unwrap()
    }

    fn command(raw: &str) -> CommandLine {
        CommandLine::parse(raw).unwrap()
    }

    fn service(raw: &str, runtime: Runtime, path: &str, line: &str) -> Service {
        Service::new(
            name(raw),
            runtime,
            RelativePath::parse(path).unwrap(),
            command(line),
        )
    }

    fn specs_for(root: &Path, services: Vec<Service>, targets: &[&str]) -> Vec<LaunchSpec> {
        let workspace = RepoWorkspace::from_parts(root.to_path_buf(), services, [], []).unwrap();
        let plan = RunPlan::resolve(&workspace, &TargetExpr::parse(targets).unwrap()).unwrap();
        launch_specs(&plan, root)
    }

    fn python(root: &Path, line: &str, prepare: &[&str]) -> LaunchSpec {
        let kg = service("kg", Runtime::Python, "apps/kg", line)
            .with_prepare(prepare.iter().map(|step| command(step)));
        specs_for(root, vec![kg], &["kg"]).remove(0)
    }

    fn with_venv(root: &Path) -> PathBuf {
        let bin = root.join("apps/kg/.venv/bin");
        fs::create_dir_all(&bin).unwrap();
        bin
    }

    #[test]
    fn maps_cwd_env_ports_prepare_and_command_from_the_service() {
        let root = Path::new("/repo");
        let api = service("api", Runtime::Node, "apps/api", "npm run dev")
            .with_ports([Port::new(3000).unwrap(), Port::new(9229).unwrap()])
            .with_prepare([command("npm run build:deps")])
            .with_env([(EnvKey::parse("NODE_ENV").unwrap(), "development".to_owned())]);

        let specs = specs_for(root, vec![api], &["api"]);

        assert_eq!(
            specs,
            vec![LaunchSpec {
                id: ServiceId::new("api"),
                cwd: PathBuf::from("/repo/apps/api"),
                command: "npm run dev".to_owned(),
                env: BTreeMap::from([("NODE_ENV".to_owned(), "development".to_owned())]),
                path_prefix: Vec::new(),
                prepare: vec!["npm run build:deps".to_owned()],
                ports: vec![3000, 9229],
                needs: Vec::new(),
            }]
        );
    }

    #[test]
    fn keeps_only_needs_that_are_part_of_the_plan() {
        let root = Path::new("/repo");
        let services = vec![
            service(
                "mongo",
                Runtime::Compose,
                "apps/kg",
                "docker compose up mongo",
            ),
            service(
                "neo4j",
                Runtime::Compose,
                "apps/kg",
                "docker compose up neo4j",
            ),
            service("kg", Runtime::Custom, "apps/kg", "make run")
                .with_needs([name("mongo"), name("neo4j")]),
        ];

        let specs = specs_for(root, services, &["kg", "-mongo"]);

        let ids: Vec<&str> = specs.iter().map(|spec| spec.id.as_str()).collect();
        assert_eq!(ids, ["neo4j", "kg"]);
        assert_eq!(specs[1].needs, [ServiceId::new("neo4j")]);
    }

    #[test]
    fn python_with_a_venv_runs_its_bin_directly_instead_of_poetry_or_uv() {
        let root = tempfile::tempdir().unwrap();
        let bin = with_venv(root.path());

        let spec = python(
            root.path(),
            "poetry run uvicorn app:app --reload",
            &["uv run alembic upgrade head", "make seed"],
        );

        assert_eq!(spec.command, "uvicorn app:app --reload");
        assert_eq!(spec.prepare, ["alembic upgrade head", "make seed"]);
        assert_eq!(spec.path_prefix, [bin]);
    }

    #[test]
    fn python_with_a_venv_strips_a_leading_uv_run_from_the_command() {
        let root = tempfile::tempdir().unwrap();
        with_venv(root.path());

        let spec = python(root.path(), "uv run python api.py", &[]);

        assert_eq!(spec.command, "python api.py");
    }

    #[test]
    fn python_without_a_venv_keeps_the_runner_and_the_inherited_path() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("apps/kg")).unwrap();

        let spec = python(
            root.path(),
            "poetry run uvicorn app:app",
            &["poetry run alembic upgrade head"],
        );

        assert_eq!(spec.command, "poetry run uvicorn app:app");
        assert_eq!(spec.prepare, ["poetry run alembic upgrade head"]);
        assert!(spec.path_prefix.is_empty());
    }

    #[test]
    fn non_python_services_ignore_a_venv() {
        let root = tempfile::tempdir().unwrap();
        with_venv(root.path());
        let tool = service("kg", Runtime::Custom, "apps/kg", "poetry run task");

        let spec = specs_for(root.path(), vec![tool], &["kg"]).remove(0);

        assert_eq!(spec.command, "poetry run task");
        assert!(spec.path_prefix.is_empty());
    }
}
