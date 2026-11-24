# cargo-depform

Dependency graph architecture checker for Cargo workspaces.

`cargo-depform` loads the workspace members, builds the graph of dependency
declarations that stay inside the workspace, and checks declarative rules from
`depform.toml` against that graph:

    cargo run -q -p cargo-depform -- graph
    cargo run -q -p cargo-depform -- check

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
its kinds, regardless of the configured dependency kinds:

    packages: 3
      app
      core
      util
    edges: 2
      app -> core (normal, dev)
      app -> util (build)

## `check`

`check` reads `depform.toml` at the workspace root, or the file named by
`--config`. Both subcommands take `--manifest-path` and pass it to
`cargo metadata`; the default is the current directory.

Exit codes: `0` passed, `1` violations, `2` a config, metadata or graph error.
Violations go to stderr as one block per violation, then a summary line.

## Configuration

```toml
[graph]
dependency-kinds = ["normal"]     # the default; also `dev` and `build`

[rules.core-isolation]
select = "client-*"
check.dependencies.deny = "core"
```

`[graph] dependency-kinds` chooses which declarations form the checked view.
An edge keeps only the selected kinds, so a `dev`-only edge disappears unless
`dev` is selected; every package stays in the view.

Each `[rules.NAME]` table has a `select` selector and at least one check.
Dotted keys (`check.dependencies.deny = "core"`), nested headers
(`[rules.NAME.check.dependencies]`) and inline tables are the same TOML
structure and all work. Unknown keys are hard errors at every level.

### Direct dependencies

```toml
[rules.core-isolation]
select = "client-*"
check.dependencies.allow = "core"
check.dependencies.deny = "legacy-*"
check.dependencies.require = "logging"
```

For every selected package `u`:

- `allow = A` fails for each direct dependency outside `A`.
- `deny = D` fails for each direct dependency inside `D`.
- `require = R` fails for each package in `R` that `u` does not depend on.

The assertions are independent and conjunctive: `allow` is not an exception to
`deny`, and one edge can break both. Details name the offending edge with its
kinds, for example `dependency client -> core (normal, dev)`.
