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
select = "@clients"
check.dependencies.allow = "@core"
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

### Transitive reachability

```toml
[rules.core-isolation]
select = "@clients"
check.reachable.allow = "@core"
```

`reachable` applies the same three assertions to the packages reachable from
`u` by one or more edges rather than to its direct dependencies. The selected
package itself is not in the set unless a cycle leads back to it. Details name
a witness path, for example `path app -> middle -> core`, so a transitive
violation is traceable to a concrete chain.

`reachable` and `dependencies` are independent: an edge can break both, and
each violation names its own assertion.

### Depth and degrees

```toml
[graph]
roots = ["app", "service"]      # optional

[rules.layering]
select = "shared-*"
check.depth.max = 2
check.dependency-count.max = 8
check.dependent-count.min = 1
```

`depth` is the shortest distance from any root, roots sitting at 0. Without
`[graph] roots`, the roots are the packages that no member depends on; that
inference only runs when a rule checks `depth`, and a view where every package
has a dependent is a graph error asking for explicit roots. Declared roots are
always validated against the graph.

`dependency-count` and `dependent-count` count distinct direct neighbors in the
view, so a filtered-out kind does not count.

`height` is the longest downstream path length in edges; a package with no
dependencies has height 0. The metric exists only in an acyclic downstream
cone: if a cycle is reachable from the package, the height is undefined and the
check fails as a graph error naming the package and the cycle. A violation
names the longest chain, for example `witness: app -> facade -> core`.

Every metric check is a range: `min`, `max`, `exact`, or `min` and `max`,
inclusive, with non-negative values. `exact` cannot be combined with the bounds.
A package no root reaches has no depth: `min` passes it, `max` and `exact` fail
it, and the detail reads `observed: unreachable`. A depth violation names its
witness, for example `app (0) -> facade (1) -> core (2)`.

### Pair distance

```toml
[rules.app-to-core]
select.from = "app"
select.to = "@core"
check.distance.exact = 2
```

A pair selector names a `from` and a `to` name selector and yields every
ordered pair except a package paired with itself. A rule holding a pair
selector may only carry pair checks, and a pair check needs a pair selector.
Pair selectors are only valid as a rule's `select`: `allow`, `deny`, `require`
and `through` take name selectors.

`distance` is the shortest directed path length between the two packages:

| range       | what it asserts                                      |
| ----------- | ---------------------------------------------------- |
| `min = 1`   | nothing: an unreachable pair has no distance and passes |
| `min = 2`   | `to` is not a direct dependency of `from`            |
| `max = 1`   | `to` is a direct dependency of `from`                |
| `exact = 1` | `to` is a direct dependency of `from`                |

An unreachable pair has no distance, so `min` passes it while `max` and `exact`
fail it with `observed: unreachable`. A violation lists `from` and `to`
separately and names a witness path such as `app -> facade -> core`.

### Gateway paths

```toml
[rules.app-through-facade]
select.from = "app"
select.to = "@core"
check.path.through = "@facade"
```

`path` asserts that *every* path from `from` to `to` passes through at least
one package of `through`. An unreachable pair has no path and therefore
passes, and the endpoints never count as intermediates, so a gateway equal to
`from` or `to` gates nothing. A violation names the shortest path that avoids
the gateway: `observed: bypass app -> core`.

`distance` and `path` may appear in the same rule; per pair, `distance` is
evaluated first.

### Selectors

A selector is a string: a package name, or a pattern where `*` matches any run
of characters (including none). Characters are ASCII alphanumerics, `-`, `_`
and `*`; matching is case-sensitive and covers the whole package name.

```toml
[groups]
core = ["*-core", "shared-*"]
clients = ["game-client", "web-*"]

[rules.core-isolation]
select = ["@clients", "tools"]
check.dependencies.allow = "@core"
```

- `@name` references a group declared in `[groups]`. A group's entries are
  patterns, never group references.
- An array is a union of at least one string selector; it may mix patterns and
  group references.
- A selector that matches no package in the view is a config error naming the
  rule, and a union is validated entry by entry. A group is validated as a
  whole, so one stale pattern inside an otherwise matching group is tolerated.
  An unused group is never checked against the graph.

A table selector selects by graph position:

```toml
[rules.consumers]
select.dependents-of = "@core"        # packages with an edge to a core package
check.depth.max = 2

[rules.layering]
select.reachable-from = "app"         # everything app reaches transitively
check.dependencies.deny = "legacy-*"
```

- `dependents-of S` selects the direct consumers of `S`, `dependencies-of S`
  its direct dependencies, and `reachable-from S` everything reachable from
  `S` by one or more edges.
- The argument must be a name selector: a string or an array of strings.
  Tables nested inside tables are a config error.
- The argument is validated like any other explicit selector, while the derived
  selection may be empty: the consumers of a package nobody depends on form an
  empty set, and its assertions then hold vacuously.
