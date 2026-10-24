# Style rules

`cargo-stylecheck` is the executable and authoritative description of the
layout rules it enforces: Rust source layout, Cargo manifest layout and TOML
file format. Run it from the tree root; reports go to the standard error
stream in `codespan-reporting` style — a `path:line:column` locus, the
offending span underlined (the whole line, or exactly the violating columns
when the rule knows them) with one neighboring line on either side, and
rule-specific observed detail as a note. The crate is also a library:
`check_tree` performs the whole check for an arbitrary root, and the command
only selects that root (the optional first argument, else the current
directory) and the config (`--config FILE` overrides the `stylecheck.toml` at
the scanned root and must name an existing file; `--help` and `--version`
work as usual, and `cargo stylecheck …` behaves like the direct invocation).

## Rule registry

Rules are grouped by what they inspect. A rule id is its group letter followed
by the rule's 1-based ordinal inside the group — `S` source layout, `M` Cargo
manifest layout, `T` TOML file format. Reports print the id with the rule's
canonical name (`error[S1 module-doc-spacing]: …`), and that canonical name
is also the section key of a rule that has options in `stylecheck.toml`, the one
config for all style options. The top-level `exempt-paths` array of that config
lists the path fragments the source-layout rules skip. The top-level
`scan-paths` array lists the trees the walk descends into, relative to the
scanned root (default: the root itself; `target/` and hidden directories are
never descended into). The ordered `[[dependency-group-order.aspects]]` array
names the root dependency aspects and gives each the path prefixes that assign
a workspace member to it. A known section rejects unknown keys — a misspelled
option is a hard config error — while an unknown top-level section is
tolerated, so a newer config can carry a section an older checker does not
know.

### Source layout — S

- **`S1` `module-doc-spacing`.** A contiguous module-level `//!` block is
  followed by exactly one blank line before the next item. Inner attributes
  (`#![doc = …]`, `#![allow(…)]`, …) and plain `//` comments under the docs
  belong to the same module header and do not count as that item. Paths
  listed in the top-level `exempt-paths` array are skipped, which is how
  standalone trybuild programs under `tests/ui/` stay exempt: their doc
  comments introduce the failing source directly.
- **`S2` `test-mod-attribute-spacing`.** `#[cfg(test)]` is immediately followed
  by its `mod` declaration.
- **`S3` `test-mod-body-spacing`.** The unit-test module body opens without a
  blank line.
- **`S4` `qualified-name`.** A usage position must not spell a `module::Type`
  path — a snake_case module segment immediately followed by a PascalCase type
  segment (`crate::identity::SchemaDescriptor`, `registry::DuplicateSchema`).
  Import the type and use its bare name instead, or allow the literal spelling
  explicitly. Out of scope: `use`/`pub use` items (and re-exports), macro
  template bodies (`quote!{…}`, `macro_rules!…`), and paths whose first segment
  is an exempt crate — the standard prelude (`std`, `core`, `alloc`,
  `proc_macro`) or a dependency key that is not itself a workspace member. A
  path rooted at a workspace member crate is checked like any other path, so the
  member type is imported rather than spelled out. Keywords and primitives are
  never module segments (`crate::Type`, `u32::MAX`, `Self::…`). The rule
  examines literal token paths only; it does not resolve `use` aliases, so an
  alias spelling such as `wire::Stream` is checked as written. Like
  `module-doc-spacing`, it skips the paths listed in `exempt-paths`. The policy
  lives in the `stylecheck.toml` of the scanned root; the rule reads its
  `[qualified-name]` table: `whitelist` and `blacklist` arrays of canonical
  spellings, where `"P::*"` is a prefix match, `"*::P"` a suffix match, and
  anything else exact. Precedence is whitelist > blacklist > built-in
  exemptions > default deny; a missing file is valid, a malformed file or an
  unsupported spelling fails the check.
- **`S5` `mod-before-use`.** Every file-module declaration (`mod name;`,
  including `pub(crate) mod name;`) precedes the file's first `use` item,
  including `pub use` re-exports. Inline modules (`mod name { … }`) are exempt:
  they are definitions the file hosts in place, so the trailing
  `#[cfg(test)] mod tests { … }` block and an in-place `pub mod prelude { … }`
  may follow the items they serve. Only unindented items are observed; nested
  items live inside an inline module or a function body.

### Cargo manifest layout — M

- **`M1` `manifest-parse`.** A `Cargo.toml` that does not parse as TOML is
  reported, and the structural rules below are skipped for it; the TOML
  file-format rules still apply.
- **`M2` `section-order`.** Direct sections appear in this order: `[package]`,
  `[lib]`, `[features]`, `[dependencies]`, `[build-dependencies]`,
  `[dev-dependencies]`, `[lints]`. Nested tables such as `[package.metadata]`
  are not section headers.
- **`M3` `section-spacing`.** Adjacent sections have exactly one blank line
  between the last content line and the next header.
- **`M4` `dependency-group-order`.** Every dependency table keeps the
  blank-delimited groups already present in the source, and each group holds
  exactly one aspect. A crate manifest orders its workspace-member dependencies
  before third-party ones; the root workspace dependency table orders the
  aspects declared by `[[dependency-group-order.aspects]]`, then third-party.
  A workspace member's aspect is the first declared aspect whose `paths` prefix
  matches the member's declared workspace path; a member matching none still
  counts as a workspace member but ranks with third-party dependencies. Entries
  such as `proc-macro2`, `quote`, and `syn` are ordinary third-party entries.
- **`M5` `dependency-sorting`.** Every dependency group is sorted.

### TOML file format — T

- **`T1` `line-endings`.** Every TOML file the walk reaches terminates every
  line with LF; CRLF is rejected. One violation per file is anchored at the
  first carriage return, so a CRLF file does not repeat the report on every
  line. A manifest that also fails to parse reports both findings.
- **`T2` `trailing-blank-line`.** A TOML file must not end with a trailing blank
  line after its last content line.
- **`T3` `missing-trailing-newline`.** A TOML file must carry the final newline
  that closes its last line. A CRLF file is left to `T1` so it is not reported
  twice.

The walk covers the TOML files at the scanned root plus every Rust source and
TOML file below each configured `scan-paths` entry, so one configuration
selects both the crates and the authored data. Rule ids, canonical names and
short report messages are defined by the checker's `Rule` registry; this README
is the single comprehensive policy reference.
