#![allow(dead_code, clippy::unwrap_used)]

use std::path::PathBuf;

use devrun_domain::{
    AliasName, CommandLine, Port, ProfileName, RelativePath, RepoWorkspace, Runtime, Service,
    ServiceName,
};

pub fn name(raw: &str) -> ServiceName {
    ServiceName::parse(raw).unwrap()
}

pub fn alias(raw: &str) -> AliasName {
    AliasName::parse(raw).unwrap()
}

pub fn profile(raw: &str) -> ProfileName {
    ProfileName::parse(raw).unwrap()
}

pub fn port(value: u16) -> Port {
    Port::new(value).unwrap()
}

pub fn root() -> PathBuf {
    PathBuf::from("/repo")
}

pub fn service(raw: &str) -> Service {
    Service::new(
        name(raw),
        Runtime::Node,
        RelativePath::parse(&format!("apps/{raw}")).unwrap(),
        CommandLine::parse("npm run dev").unwrap(),
    )
}

pub fn service_needing(raw: &str, needs: &[&str]) -> Service {
    service(raw).with_needs(needs.iter().map(|need| name(need)))
}

pub fn jaspy_like() -> RepoWorkspace {
    RepoWorkspace::from_parts(
        root(),
        [
            service("web").with_ports([port(5173)]),
            service("api").with_ports([port(3000)]),
            service("mongo"),
            service("neo4j"),
            service_needing("knowledge-graph", &["mongo", "neo4j"]).with_ports([port(8081)]),
        ],
        [
            (alias("kg"), name("knowledge-graph")),
            (alias("fe"), name("web")),
        ],
        [(profile("fullstack"), vec![name("web"), name("api")])],
    )
    .unwrap()
}
