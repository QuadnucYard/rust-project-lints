# https://just.systems

[windows]
set shell := ["nu", "-c"]

alias c := check
alias f := fmt
alias t := test

fmt *ARGS:
    cargo +nightly fmt --all {{ARGS}}

check *ARGS:
    cargo clippy --locked --workspace --all-targets --all-features -- {{ARGS}}

doc *ARGS:
    cargo doc --locked --workspace --no-deps {{ARGS}}

test *ARGS:
    cargo nextest run --no-fail-fast --status-level=fail --locked {{ARGS}}

style-check:
    cargo run --locked -q -p cargo-stylecheck

# Check every tracked file for misspellings under the repo dictionaries.
typos *ARGS:
    typos {{ARGS}}

ci:
    @just typos
    @just fmt -- --check
    @just check -D warnings
    @just doc
    @just test
    @just style-check
