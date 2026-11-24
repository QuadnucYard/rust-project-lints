//! The workspace projection: which members become nodes and which
//! declarations become edges.

mod support;

use std::{collections::BTreeSet, fs, process::Command};

use depform::{DependencyKind, load_workspace};
use support::Fixture;

/// What `graph` prints for the three-member fixture below.
const EXPECTED_GRAPH: &str = "\
packages: 3
  beta
  game-alpha
  gamma
edges: 4
  beta -> game-alpha (normal, build)
  beta -> gamma (normal)
  gamma -> beta (normal)
  gamma -> game-alpha (normal, dev)
";

fn kinds(values: &[DependencyKind]) -> BTreeSet<DependencyKind> {
    values.iter().copied().collect()
}

/// Three members: `beta` normally depends on `game-alpha` and on an external
/// crate, optionally on `gamma`, and build-depends on `game-alpha`; `gamma`
/// dev-depends on `game-alpha`, optionally renames it as a normal dependency,
/// and target-depends on `beta`.
fn workspace() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.member("game-alpha", "");
    fixture.member(
        "beta",
        "[dependencies]\n\
         game-alpha.path = \"../game-alpha\"\n\
         gamma = { path = \"../gamma\", optional = true }\n\
         serde = \"1\"\n\n\
         [build-dependencies]\ngame-alpha.path = \"../game-alpha\"\n",
    );
    fixture.member(
        "gamma",
        "[dependencies]\n\
         renamed = { path = \"../game-alpha\", package = \"game-alpha\", optional = true }\n\n\
         [dev-dependencies]\ngame-alpha.path = \"../game-alpha\"\n\n\
         [target.'cfg(unix)'.dependencies]\nbeta.path = \"../beta\"\n",
    );
    fixture
}

#[test]
fn members_become_nodes_and_internal_declarations_become_edges() {
    let fixture = workspace();
    let workspace =
        load_workspace(Some(&fixture.manifest_path())).expect("load the fixture workspace");
    let graph = workspace.graph();

    assert_eq!(graph.package_count(), 3);
    assert_eq!(
        graph.packages().collect::<Vec<_>>(),
        ["beta", "game-alpha", "gamma"]
    );
    assert_eq!(graph.edge_count(), 4);
    assert_eq!(
        graph
            .edges()
            .map(|(from, to, kinds)| (from.to_owned(), to.to_owned(), kinds.clone()))
            .collect::<Vec<_>>(),
        vec![
            (
                "beta".to_owned(),
                "game-alpha".to_owned(),
                kinds(&[DependencyKind::Normal, DependencyKind::Build]),
            ),
            (
                "beta".to_owned(),
                "gamma".to_owned(),
                kinds(&[DependencyKind::Normal]),
            ),
            (
                "gamma".to_owned(),
                "beta".to_owned(),
                kinds(&[DependencyKind::Normal]),
            ),
            (
                "gamma".to_owned(),
                "game-alpha".to_owned(),
                kinds(&[DependencyKind::Normal, DependencyKind::Dev]),
            ),
        ]
    );

    let root = fs::canonicalize(workspace.root()).expect("canonical workspace root");
    let expected = fs::canonicalize(fixture.root()).expect("canonical fixture root");
    assert_eq!(root, expected);
}

#[test]
fn a_registry_dependency_sharing_a_member_name_is_not_an_edge() {
    let mut fixture = Fixture::new();
    fixture.member("serde", "");
    fixture.member("app", "[dependencies]\nserde = \"1\"\n");
    let workspace =
        load_workspace(Some(&fixture.manifest_path())).expect("load the fixture workspace");
    assert_eq!(workspace.graph().edge_count(), 0);
}

#[test]
fn graph_command_prints_the_contract_format() {
    let fixture = workspace();
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-depform"))
        .args(["graph", "--manifest-path"])
        .arg(fixture.manifest_path())
        .output()
        .expect("run cargo-depform graph");

    assert!(
        output.status.success(),
        "graph failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), EXPECTED_GRAPH);
}
