//! The `check` subcommand: exit codes, streams and the diagnostic format.

mod support;

use std::{fs, process::Command};

use support::Fixture;

/// A two-member workspace where `client` normally depends on `core`.
fn workspace() -> Fixture {
    let mut fixture = Fixture::new();
    fixture.member("core", "");
    fixture.member("client", "[dependencies]\ncore.path = \"../core\"\n");
    fixture
}

/// A rule denying the only internal edge of the fixture.
const DENYING_CONFIG: &str = "\
[rules.core-isolation]
select = \"client\"
check.dependencies.deny = \"core\"
";

#[test]
fn a_violation_renders_its_block_and_fails() {
    let fixture = workspace();
    fixture.write("depform.toml", DENYING_CONFIG);
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-depform"))
        .args(["check", "--manifest-path"])
        .arg(fixture.manifest_path())
        .output()
        .expect("run cargo-depform check");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "error[core-isolation]: package `client` has a denied dependency `core`\n  \
         selection: \"client\"\n  \
         package: client\n  \
         observed: dependency client -> core (normal)\n  \
         expected: deny \"core\"\n\
         \n\
         depform check failed: 1 violation\n"
    );
}

#[test]
fn a_passing_check_prints_the_pass_line() {
    let fixture = workspace();
    fixture.write(
        "depform.toml",
        "[rules.core-isolation]\nselect = \"client\"\ncheck.dependencies.allow = \"core\"\n",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-depform"))
        .args(["check", "--manifest-path"])
        .arg(fixture.manifest_path())
        .output()
        .expect("run cargo-depform check");

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "depform check passed\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn a_missing_default_config_is_reported_as_such() {
    let fixture = workspace();
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-depform"))
        .args(["check", "--manifest-path"])
        .arg(fixture.manifest_path())
        .output()
        .expect("run cargo-depform check");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cargo-depform: no `depform.toml` at `"),
        "{stderr}"
    );
    assert!(
        stderr.contains("; create one or pass `--config`\n"),
        "{stderr}"
    );
}

#[test]
fn an_explicit_config_path_overrides_the_default() {
    let fixture = workspace();
    let config = fixture.root().join("checks").join("custom.toml");
    fixture.write("checks/custom.toml", DENYING_CONFIG);
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-depform"))
        .args(["check", "--manifest-path"])
        .arg(fixture.manifest_path())
        .arg("--config")
        .arg(&config)
        .output()
        .expect("run cargo-depform check");

    assert_eq!(
        output.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_missing_explicit_config_is_an_io_error() {
    let fixture = workspace();
    let missing = fixture.root().join("absent.toml");
    assert!(!fs::exists(&missing).expect("probe the config path"));
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-depform"))
        .args(["check", "--manifest-path"])
        .arg(fixture.manifest_path())
        .arg("--config")
        .arg(&missing)
        .output()
        .expect("run cargo-depform check");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("cargo-depform: failed to read `"),
        "{stderr}"
    );
}

#[test]
fn a_broken_workspace_is_a_metadata_error() {
    let fixture = workspace();
    let manifest = fixture.root().join("missing").join("Cargo.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_cargo-depform"))
        .args(["check", "--manifest-path"])
        .arg(&manifest)
        .output()
        .expect("run cargo-depform check");

    assert_eq!(
        output.status.code(),
        Some(2),
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.starts_with("cargo-depform: "), "{stderr}");
    assert!(stderr.contains("cargo metadata"), "{stderr}");
    assert!(!stderr.contains("depform.toml"), "{stderr}");
}
