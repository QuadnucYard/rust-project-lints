use super::*;
use crate::{DependencyKind, config::RuleConfig};

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn edge(from: &str, to: &str, kind: DependencyKind) -> (String, String, DependencyKind) {
    (from.to_owned(), to.to_owned(), kind)
}

/// `app` depends on `core` and `shared`; `isolated` has no edges.
fn app_graph() -> Graph {
    Graph::new(
        names(&["app", "core", "shared", "isolated"]),
        [
            edge("app", "core", DependencyKind::Normal),
            edge("app", "shared", DependencyKind::Normal),
        ],
    )
}

/// `app -> middle -> core`, plus a direct `app -> shared` edge and an
/// unconnected `isolated` package.
fn chain_graph() -> Graph {
    Graph::new(
        names(&["app", "middle", "core", "shared", "isolated"]),
        [
            edge("app", "middle", DependencyKind::Normal),
            edge("app", "shared", DependencyKind::Normal),
            edge("middle", "core", DependencyKind::Normal),
        ],
    )
}

fn parse_config(text: &str) -> Config {
    Config::from_toml(text).expect("a valid config")
}

fn sel(pattern: &str) -> Selector {
    Selector::Name(pattern.to_owned())
}

fn node_with(name: &str, select: &str, edit: impl FnOnce(&mut NodeRule)) -> RuleConfig {
    RuleConfig::node_with(name, sel(select), edit)
}

/// A set check with the given selectors.
fn set(allow: Option<&str>, deny: Option<&str>, require: Option<&str>) -> SetCheck {
    SetCheck {
        allow: allow.map(sel),
        deny: deny.map(sel),
        require: require.map(sel),
    }
}

fn keys(violation: &Violation) -> Vec<&'static str> {
    violation.details().iter().map(|(key, _)| *key).collect()
}

fn value<'a>(violation: &'a Violation, key: &str) -> &'a str {
    violation
        .details()
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, value)| value.as_str())
        .expect("a detail")
}

#[test]
fn deny_reports_each_denied_edge() {
    let rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, Some("core"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&app_graph(), &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(
        violations[0].message(),
        "package `app` has a denied dependency `core`"
    );
    assert_eq!(
        keys(&violations[0]),
        ["selection", "package", "observed", "expected"]
    );
    assert_eq!(value(&violations[0], "selection"), "\"app\"");
    assert_eq!(value(&violations[0], "package"), "app");
    assert_eq!(
        value(&violations[0], "observed"),
        "dependency app -> core (normal)"
    );
    assert_eq!(value(&violations[0], "expected"), "deny \"core\"");
}

#[test]
fn allow_reports_dependencies_outside_the_set() {
    let rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(Some("core"), None, None));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&app_graph(), &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(
        violations[0].message(),
        "package `app` has a dependency outside the allowed set: `shared`"
    );
    assert_eq!(value(&violations[0], "expected"), "allow \"core\"");
}

#[test]
fn allow_and_deny_are_independent_assertions() {
    let rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(Some("core"), Some("shared"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&app_graph(), &config).expect("a check");
    let messages: Vec<&str> = violations.iter().map(Violation::message).collect();
    assert_eq!(
        messages,
        [
            "package `app` has a dependency outside the allowed set: `shared`",
            "package `app` has a denied dependency `shared`",
        ]
    );
}

#[test]
fn require_reports_each_missing_dependency() {
    let rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, None, Some("isolated")));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&app_graph(), &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(
        violations[0].message(),
        "package `app` is missing required dependency `isolated`"
    );
    assert_eq!(
        value(&violations[0], "observed"),
        "missing dependency isolated"
    );
    assert_eq!(value(&violations[0], "expected"), "require \"isolated\"");

    let satisfied_rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, None, Some("core")));
    });
    let satisfied = Config::from_rules(vec![satisfied_rule]);
    assert!(
        check_graph(&app_graph(), &satisfied)
            .expect("a check")
            .is_empty()
    );
}

#[test]
fn rules_are_reported_in_name_order() {
    let config = parse_config(
        "[rules.r]\nselect = \"app\"\ncheck.dependencies.deny = \"core\"\n\n\
             [rules.a]\nselect = \"app\"\ncheck.dependencies.deny = \"shared\"\n",
    );
    let violations = check_graph(&app_graph(), &config).expect("a check");
    let rules: Vec<&str> = violations.iter().map(Violation::rule).collect();
    assert_eq!(rules, ["a", "r"]);
}

#[test]
fn observed_edges_carry_every_selected_kind() {
    let graph = Graph::new(
        names(&["app", "core"]),
        [
            edge("app", "core", DependencyKind::Normal),
            edge("app", "core", DependencyKind::Dev),
        ],
    );
    let rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, Some("core"), None));
    });
    let config =
        Config::from_rules(vec![rule]).with_kinds([DependencyKind::Normal, DependencyKind::Dev]);
    let violations = check_graph(&graph, &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(
        value(&violations[0], "observed"),
        "dependency app -> core (normal, dev)"
    );
}

#[test]
fn unselected_dependency_kinds_do_not_form_edges() {
    let graph = Graph::new(
        names(&["app", "core"]),
        [edge("app", "core", DependencyKind::Dev)],
    );
    let rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, Some("core"), None));
    });
    let normal_only = Config::from_rules(vec![rule]);
    assert!(
        check_graph(&graph, &normal_only)
            .expect("a check")
            .is_empty()
    );
    let dev_rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, Some("core"), None));
    });
    let dev = Config::from_rules(vec![dev_rule]).with_kinds([DependencyKind::Dev]);
    assert_eq!(check_graph(&graph, &dev).expect("a check").len(), 1);
}

#[test]
fn an_explicit_selector_that_matches_nothing_is_a_config_error() {
    let rule = node_with("r", "nope", |rule| {
        rule.dependencies = Some(set(None, Some("core"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let error = check_graph(&app_graph(), &config)
        .expect_err("a config error")
        .to_string();
    assert!(error.contains("rule `r`"), "{error}");
    assert!(error.contains("\"nope\""), "{error}");

    let check_selector = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, Some("nope"), None));
    });
    let config = Config::from_rules(vec![check_selector]);
    let error = check_graph(&app_graph(), &config)
        .expect_err("a config error")
        .to_string();
    assert!(error.contains("rule `r`"), "{error}");
    assert!(error.contains("\"nope\""), "{error}");
}

#[test]
fn packages_and_dependencies_are_reported_in_sorted_order() {
    let graph = Graph::new(
        names(&["app", "zulu", "alpha"]),
        [
            edge("app", "zulu", DependencyKind::Normal),
            edge("app", "alpha", DependencyKind::Normal),
            edge("zulu", "alpha", DependencyKind::Normal),
        ],
    );
    let rule = node_with("r", "app", |rule| {
        rule.select = Selector::Union(vec![sel("zulu"), sel("app")]);
        rule.dependencies = Some(set(None, Some("*"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&graph, &config).expect("a check");
    let observed: Vec<&str> = violations
        .iter()
        .map(|violation| value(violation, "observed"))
        .collect();
    assert_eq!(
        observed,
        [
            "dependency app -> alpha (normal)",
            "dependency app -> zulu (normal)",
            "dependency zulu -> alpha (normal)",
        ]
    );
}

#[test]
fn reachable_deny_reports_the_witness_path() {
    let rule = node_with("r", "app", |rule| {
        rule.reachable = Some(set(None, Some("core"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&chain_graph(), &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(
        violations[0].message(),
        "package `app` reaches denied package `core`"
    );
    assert_eq!(
        value(&violations[0], "observed"),
        "path app -> middle -> core"
    );
    assert_eq!(value(&violations[0], "expected"), "deny \"core\"");
}

#[test]
fn reachable_deny_reports_a_cycle_back_to_the_package() {
    let graph = Graph::new(
        names(&["a", "b"]),
        [
            edge("a", "b", DependencyKind::Normal),
            edge("b", "a", DependencyKind::Normal),
        ],
    );
    let rule = node_with("r", "a", |rule| {
        rule.reachable = Some(set(None, Some("a"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&graph, &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(
        violations[0].message(),
        "package `a` reaches denied package `a`"
    );
    assert_eq!(value(&violations[0], "observed"), "path a -> b -> a");
}

#[test]
fn reachable_allow_reports_every_reached_package_outside_the_set() {
    let rule = node_with("r", "app", |rule| {
        rule.reachable = Some(set(Some("middle"), None, None));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&chain_graph(), &config).expect("a check");
    let reached: Vec<&str> = violations
        .iter()
        .map(|violation| value(violation, "observed"))
        .collect();
    assert_eq!(
        reached,
        ["path app -> middle -> core", "path app -> shared"]
    );
    assert_eq!(
        violations[0].message(),
        "package `app` reaches `core`, which is outside the allowed set"
    );
}

#[test]
fn reachable_require_reports_missing_reachable_packages() {
    let rule = node_with("r", "app", |rule| {
        rule.reachable = Some(set(None, None, Some("isolated")));
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&chain_graph(), &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(
        violations[0].message(),
        "package `app` cannot reach required package `isolated`"
    );
    assert_eq!(
        value(&violations[0], "observed"),
        "missing reachable isolated"
    );
}

#[test]
fn reachable_checks_follow_dependency_checks_per_package() {
    let rule = node_with("r", "app", |rule| {
        rule.dependencies = Some(set(None, Some("middle"), None));
        rule.reachable = Some(set(None, Some("core"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let graph = Graph::new(
        names(&["app", "middle", "core"]),
        [
            edge("app", "middle", DependencyKind::Normal),
            edge("middle", "core", DependencyKind::Normal),
        ],
    );
    let violations = check_graph(&graph, &config).expect("a check");
    assert_eq!(
        violations
            .iter()
            .map(Violation::message)
            .collect::<Vec<_>>(),
        [
            "package `app` has a denied dependency `middle`",
            "package `app` reaches denied package `core`",
        ]
    );
}

#[test]
fn a_group_is_validated_as_a_whole() {
    let group = Selector::Group {
        name: "core".to_owned(),
        patterns: vec!["core".to_owned(), "gone*".to_owned()],
    };
    let rule = RuleConfig::node_with("r", sel("app"), |rule| {
        rule.dependencies = Some(SetCheck {
            allow: None,
            deny: Some(group),
            require: None,
        });
    });
    let config = Config::from_rules(vec![rule]);
    let violations = check_graph(&app_graph(), &config).expect("a check");
    assert_eq!(violations.len(), 1);
    assert_eq!(value(&violations[0], "expected"), "deny @core");
}

#[test]
fn a_referenced_group_matching_nothing_is_a_config_error() {
    let stale = Selector::Group {
        name: "stale".to_owned(),
        patterns: vec!["nope*".to_owned()],
    };
    let config = Config::from_rules(vec![RuleConfig::node("r", stale)]);
    let error = check_graph(&app_graph(), &config)
        .expect_err("a config error")
        .to_string();
    assert!(error.contains("rule `r`"), "{error}");
    assert!(error.contains("@stale"), "{error}");
}

#[test]
fn an_unused_group_matching_nothing_is_accepted() {
    let config = parse_config(
        "[groups]\nunused = [\"nope*\"]\n\n\
             [rules.r]\nselect = \"app\"\ncheck.dependencies.deny = \"core\"\n",
    );
    assert_eq!(
        check_graph(&app_graph(), &config).expect("a check").len(),
        1
    );
}

#[test]
fn a_union_is_validated_entry_by_entry() {
    let union = Selector::Union(vec![
        sel("app"),
        Selector::Group {
            name: "core".to_owned(),
            patterns: vec!["core".to_owned()],
        },
        sel("gone*"),
    ]);
    let rule = RuleConfig::node_with("r", union, |rule| {
        rule.dependencies = Some(set(None, Some("core"), None));
    });
    let config = Config::from_rules(vec![rule]);
    let error = check_graph(&app_graph(), &config)
        .expect_err("a config error")
        .to_string();
    assert!(error.contains("gone*"), "{error}");
}
