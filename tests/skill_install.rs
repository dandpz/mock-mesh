#![allow(clippy::unwrap_used)]
//! Tests for `mock-mesh skill install`. Drives the library `skill::install`
//! directly (the rest of the suite is library-level too), plus a binary smoke
//! test that guards backward compatibility of the server CLI.

use std::process::Command;

use mock_mesh::cli::SkillInstallArgs;

fn args(dir: &std::path::Path, force: bool, print: bool) -> SkillInstallArgs {
    SkillInstallArgs {
        dir: Some(dir.to_path_buf()),
        force,
        print,
    }
}

#[test]
fn install_writes_skill_and_starter_files() {
    let tmp = tempfile::tempdir().unwrap();
    mock_mesh::skill::install(&args(tmp.path(), false, false)).unwrap();

    let base = tmp.path().join(".claude/skills/mock-mesh");
    let skill = base.join("SKILL.md");
    assert!(skill.exists(), "SKILL.md should exist");
    assert!(base.join("examples/openapi.yaml").exists());
    assert!(base.join("examples/mock-mesh.yaml").exists());

    let body = std::fs::read_to_string(&skill).unwrap();
    assert!(body.starts_with("---"), "frontmatter delimiter");
    assert!(
        body.contains("name: mock-mesh"),
        "skill name in frontmatter"
    );
    assert!(body.contains("description:"));
}

#[test]
fn install_refuses_existing_then_force_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    mock_mesh::skill::install(&args(tmp.path(), false, false)).unwrap();

    // Second install without --force is an error and writes nothing new.
    let err = mock_mesh::skill::install(&args(tmp.path(), false, false));
    assert!(err.is_err(), "should refuse to overwrite without --force");

    // With --force it succeeds.
    mock_mesh::skill::install(&args(tmp.path(), true, false)).unwrap();
}

#[test]
fn print_writes_nothing_to_disk() {
    let tmp = tempfile::tempdir().unwrap();
    mock_mesh::skill::install(&args(tmp.path(), false, true)).unwrap();
    assert!(
        !tmp.path().join(".claude").exists(),
        "--print must not touch the filesystem"
    );
}

/// Backward-compat guard: the binary still runs server-mode flags, and the
/// new subcommand exits cleanly.
#[test]
fn binary_subcommand_and_server_flags_coexist() {
    let bin = env!("CARGO_BIN_EXE_mock-mesh");
    let tmp = tempfile::tempdir().unwrap();

    let install = Command::new(bin)
        .args(["skill", "install", "--dir"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(install.status.success(), "skill install should exit 0");
    assert!(
        tmp.path()
            .join(".claude/skills/mock-mesh/SKILL.md")
            .exists(),
        "binary install should write the skill"
    );

    // `--spec ... --validate` (no subcommand) still works as before.
    let validate = Command::new(bin)
        .args(["--spec", "tests/fixtures/petstore.yaml", "--validate"])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "server --validate must still exit 0"
    );

    // No --spec, no subcommand => clear failure, not a panic.
    let missing = Command::new(bin).output().unwrap();
    assert!(!missing.status.success(), "missing --spec should fail");
}
