# cargo-depform

Dependency graph architecture checker for Cargo workspaces.

`cargo-depform` loads the workspace members and builds the graph of dependency
declarations that stay inside the workspace:

    cargo run -q -p cargo-depform -- graph

## Graph scope

- Nodes are the package names of workspace members, taken from
  `cargo metadata --no-deps` with dashes preserved. The crate identifier of
  `game-client` is `game_client`, but the node is named `game-client`.
- Edges are dependency declarations whose target is another workspace member.
  A declaration that points at an external crate is ignored: the graph is
  workspace-only, so `allow` and `deny` never see third-party crates.
- Declarations of every kind are recorded, and each edge keeps the set of
  kinds that declared it. A renamed declaration still targets the package it
  names, not the local alias.
- Optional and target-specific declarations are included; the graph describes
  declarations, not a resolved build.

## `graph`

`graph` prints every package and every declared workspace-internal edge with
its kinds:

    packages: 3
      app
      core
      util
    edges: 2
      app -> core (normal, dev)
      app -> util (build)
