# AGENTS.md

Guidance for coding agents working in this repository.

## Layout

Cargo workspace of independent lint/style tools for Rust projects. Crates are
isolated: each carries its own `version`, `edition`, and `rust-version` — there
is no `[workspace.package]` inheritance, and none should be added. Shared
dependency versions (`[workspace.dependencies]`) and lints (`[workspace.lints]`)
are inherited.

Naming convention for tools: package and bin `cargo-<tool>`, directory
`crates/<tool>`, explicit `[lib] name = "<tool>"`. Nothing is published
(`publish = false`); run tools with `cargo run -p <package>`.

## Checks

All four gates must pass before finishing any change:

    cargo fmt --all --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo test --workspace
    cargo run -q -p cargo-stylecheck

The last gate is dogfooding: this repository must pass its own style checker.
Its configuration is `stylecheck.toml` at the root; the rule registry is
documented in `crates/stylecheck/README.md`, and a unit test enforces that
every rule stays documented there — change the two together.

## Style constraints

- LF line endings everywhere (`.gitattributes` sets `eol=lf`; the checker's
  `line-endings` rule flags CRLF).
- New top-level files must be allowed in `.gitignore` — it is a whitelist
  (nothing is tracked unless explicitly allowed).
- `Cargo.toml` section order and dependency sorting are checked by the tool
  itself; keep manifests passing the style check.

## Version control

jj + git colocated repository. Mutate history with `jj` (`describe`, `new`,
`squash`) — never raw git staging or commits. Describe every change with a
Conventional Commit message.
