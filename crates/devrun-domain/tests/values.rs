#![allow(clippy::unwrap_used)]

use devrun_domain::{CommandLine, EnvKey, Port, RelativePath, Runtime, ServiceName};

#[test]
fn service_names_accept_lowercase_identifiers() {
    for raw in ["web", "knowledge-graph", "api_v2", "a.b", "9to5"] {
        assert_eq!(ServiceName::parse(raw).unwrap().as_str(), raw);
    }
}

#[test]
fn service_names_reject_invalid_identifiers_with_guidance() {
    for raw in ["", "Web", "-web", "my web", "@repo/ui", &"a".repeat(65)] {
        let error = ServiceName::parse(raw).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!(
                "invalid service name `{raw}`: use 1-64 lowercase letters, digits, `-`, `_` or `.`, starting with a letter or digit"
            )
        );
    }
}

#[test]
fn slug_turns_package_labels_into_service_names() {
    assert_eq!(
        ServiceName::slug("@repo/company-enrichment-core")
            .unwrap()
            .as_str(),
        "repo-company-enrichment-core"
    );
    assert_eq!(
        ServiceName::slug("Jaspy  Frontend").unwrap().as_str(),
        "jaspy-frontend"
    );
    assert!(ServiceName::slug("@@@").is_err());
}

#[test]
fn port_zero_is_rejected() {
    assert_eq!(
        Port::new(0).unwrap_err().to_string(),
        "invalid port `0`: ports must be between 1 and 65535"
    );
    assert_eq!(Port::new(8080).unwrap().get(), 8080);
}

#[test]
fn relative_paths_are_normalized_and_confined_to_the_repository() {
    assert_eq!(
        RelativePath::parse("./apps/web/").unwrap().to_string(),
        "apps/web"
    );
    assert_eq!(RelativePath::parse(".").unwrap().to_string(), ".");
    assert_eq!(
        RelativePath::parse("apps/../../etc")
            .unwrap_err()
            .to_string(),
        "invalid path `apps/../../etc`: must not leave the repository"
    );
    assert_eq!(
        RelativePath::parse("/etc").unwrap_err().to_string(),
        "invalid path `/etc`: must be relative to the repository root"
    );
}

#[test]
fn commands_must_not_be_blank() {
    assert_eq!(
        CommandLine::parse("  npm run dev ").unwrap().as_str(),
        "npm run dev"
    );
    assert_eq!(
        CommandLine::parse("   ").unwrap_err().to_string(),
        "command must not be empty"
    );
}

#[test]
fn env_keys_follow_shell_variable_rules() {
    assert!(EnvKey::parse("PORT").is_ok());
    assert!(EnvKey::parse("_private_1").is_ok());
    assert_eq!(
        EnvKey::parse("1PORT").unwrap_err().to_string(),
        "invalid environment variable name `1PORT`"
    );
}

#[test]
fn runtimes_round_trip_through_their_names() {
    for runtime in Runtime::ALL {
        assert_eq!(runtime.as_str().parse::<Runtime>().unwrap(), runtime);
    }
    assert_eq!(
        "ruby".parse::<Runtime>().unwrap_err().to_string(),
        "unknown runtime `ruby`: expected node, python, compose or custom"
    );
}
