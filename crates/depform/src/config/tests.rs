use super::*;

/// The message of the config error the document must produce.
fn error(text: &str) -> String {
    Config::from_toml(text)
        .expect_err("a config error")
        .to_string()
}

#[test]
fn a_minimal_document_defaults_to_normal_kinds() {
    let config = Config::from_toml(
        "[rules.isolation]\nselect = \"core\"\ncheck.dependencies.deny = \"client\"\n",
    )
    .expect("a valid config");
    assert_eq!(config.kinds(), &BTreeSet::from([DependencyKind::Normal]));
    assert_eq!(config.rules().len(), 1);
    let rule = &config.rules()[0];
    assert_eq!(rule.name, "isolation");
    let check = rule
        .node_rule()
        .dependencies
        .as_ref()
        .expect("a dependency check");
    assert!(check.allow.is_none());
    assert_eq!(
        check.deny.as_ref().map(ToString::to_string),
        Some("\"client\"".to_owned())
    );
    assert!(check.require.is_none());
}

#[test]
fn dependency_kinds_are_validated() {
    let config = Config::from_toml(
        "[graph]\ndependency-kinds = [\"build\", \"normal\"]\n\n\
             [rules.r]\nselect = \"a\"\ncheck.dependencies.deny = \"b\"\n",
    )
    .expect("a valid config");
    assert_eq!(
        config.kinds().iter().copied().collect::<Vec<_>>(),
        [DependencyKind::Normal, DependencyKind::Build]
    );
    assert!(error("[graph]\ndependency-kinds = []\n").contains("at least one kind"));
    assert!(error("[graph]\ndependency-kinds = [\"dev\", \"dev\"]\n").contains("repeats `dev`"));
    assert!(error("[graph]\ndependency-kinds = [\"nope\"]\n").contains("unknown kind `nope`"));
    assert!(error("[graph]\ndependency-kinds = [1]\n").contains("dependency-kinds"));
}

#[test]
fn unknown_keys_are_rejected_at_every_level() {
    assert!(error("nope = 1\n").contains("nope"));
    assert!(error("[graph]\nnope = 1\n").contains("nope"));
    assert!(
        error("[rules.r]\nselect = \"a\"\ncheck.dependencies.deny = \"b\"\nnope = 1\n")
            .contains("nope")
    );
    assert!(error("[rules.r]\nselect = \"a\"\ncheck.nope = 1\n").contains("unknown check `nope`"));
    assert!(
        error("[rules.r]\nselect = \"a\"\ncheck.dependencies.nope = \"b\"\n")
            .contains("unknown dependency check key `nope`")
    );
    assert!(
        error("[[rules]]\nselect = \"a\"\ncheck.dependencies.deny = \"b\"\n")
            .contains("invalid type")
    );
}

#[test]
fn reachable_checks_share_the_set_check_shape() {
    let config = Config::from_toml(
        "[rules.r]\nselect = \"a\"\ncheck.reachable.allow = \"b\"\n\
             check.reachable.require = [\"c\", \"d\"]\n",
    )
    .expect("a valid config");
    let reachable = config.rules()[0]
        .node_rule()
        .reachable
        .as_ref()
        .expect("a reachable check");
    assert_eq!(
        reachable.allow.as_ref().map(ToString::to_string),
        Some("\"b\"".to_owned())
    );
    assert_eq!(
        reachable.require.as_ref().map(ToString::to_string),
        Some("[\"c\", \"d\"]".to_owned())
    );
    assert!(
        error("[rules.r]\nselect = \"a\"\ncheck.reachable = {}\n")
            .contains("reachable check needs at least one")
    );
    assert!(
        error("[rules.r]\nselect = \"a\"\ncheck.reachable.nope = \"b\"\n")
            .contains("unknown reachable check key `nope`")
    );
    assert!(
        error("[rules.r]\nselect = \"a\"\ncheck.reachable = 1\n")
            .contains("a reachable check must be a table")
    );
}

#[test]
fn check_keys_populate_their_fields() {
    let config = Config::from_toml(
        "[rules.node]\nselect = \"a\"\n\
             check.dependencies.deny = \"b\"\n\
             check.reachable.require = \"c\"\n",
    )
    .expect("a valid config");
    let node = config.rules()[0].node_rule();
    assert_eq!(node.select.to_string(), "\"a\"");
    assert_eq!(
        node.dependencies
            .as_ref()
            .and_then(|check| check.deny.as_ref())
            .map(ToString::to_string),
        Some("\"b\"".to_owned())
    );
    assert_eq!(
        node.reachable
            .as_ref()
            .and_then(|check| check.require.as_ref())
            .map(ToString::to_string),
        Some("\"c\"".to_owned())
    );
}

#[test]
fn a_rule_needs_a_selector_and_a_check() {
    assert!(error("[rules.r]\ncheck.dependencies.deny = \"b\"\n").contains("needs a `select`"));
    assert!(error("[rules.r]\nselect = \"a\"\n").contains("check"));
    assert!(error("[rules.r]\nselect = \"a\"\ncheck = {}\n").contains("at least one check"));
    assert!(
        error("[rules.r]\nselect = \"a\"\ncheck = { dependencies = {} }\n")
            .contains("at least one of")
    );
}

#[test]
fn rule_names_use_the_identifier_alphabet() {
    assert!(
        error("[rules.\"\"]\nselect = \"a\"\ncheck.dependencies.deny = \"b\"\n")
            .contains("invalid rule name")
    );
    assert!(
        error("[rules.\"a b\"]\nselect = \"a\"\ncheck.dependencies.deny = \"b\"\n")
            .contains("invalid rule name")
    );
    assert!(
        Config::from_toml(
            "[rules.core_isolation-2]\nselect = \"a\"\ncheck.dependencies.deny = \"b\"\n"
        )
        .is_ok()
    );
}

#[test]
fn group_tables_are_validated_while_parsing() {
    assert!(error("[groups]\ncore = []\n").contains("at least one pattern"));
}
