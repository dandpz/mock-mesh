//! `mock-mesh skill install` — write the bundled Claude Code skill into a
//! repo's `.claude/skills/mock-mesh/` so an AI coding agent can drive
//! mock-mesh autonomously. All content is embedded in the binary, so
//! installed (`cargo install` / binstall) users get it with no runtime files.

use std::fs;
use std::path::PathBuf;

use crate::cli::SkillInstallArgs;
use crate::error::SkillError;

const SKILL_MD: &str = include_str!("../assets/skill/SKILL.md");
const STARTER_SPEC: &str = include_str!("../assets/skill/examples/openapi.yaml");
const STARTER_CFG: &str = include_str!("../assets/skill/examples/mock-mesh.yaml");

/// Directory, relative to the install root, that holds the skill.
const REL_DIR: &str = ".claude/skills/mock-mesh";

/// Install (or `--print`) the bundled skill.
pub fn install(args: &SkillInstallArgs) -> Result<(), SkillError> {
    if args.print {
        print!("{SKILL_MD}");
        return Ok(());
    }

    let root = args.dir.clone().unwrap_or_else(|| PathBuf::from("."));
    let dir = root.join(REL_DIR);
    let files = [
        (dir.join("SKILL.md"), SKILL_MD),
        (dir.join("examples/openapi.yaml"), STARTER_SPEC),
        (dir.join("examples/mock-mesh.yaml"), STARTER_CFG),
    ];

    // All-or-nothing: refuse before writing anything if a target exists.
    if !args.force {
        for (path, _) in &files {
            if path.exists() {
                return Err(SkillError::Exists { path: path.clone() });
            }
        }
    }

    for (path, body) in &files {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| SkillError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(path, body).map_err(|source| SkillError::Io {
            path: path.clone(),
            source,
        })?;
    }

    println!("installed mock-mesh skill -> {}", dir.display());
    println!("Reload Claude Code to pick it up, then ask it to mock an API.");
    Ok(())
}
