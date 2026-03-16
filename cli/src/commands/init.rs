//! `dr init` — initialise the current directory as a Daram project.

use std::env;

use crate::{
    manifest::{
        validate_package_name, BuildConfig, DocConfig, FmtConfig, LintConfig, Manifest, PackageMeta,
    },
    terminal,
    workspace::create_project_skeleton,
};

pub fn run(_args: &[String]) -> i32 {
    let cwd = match env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            terminal::error(&format!("cannot determine current directory: {}", e));
            return 1;
        }
    };

    // Refuse to overwrite an existing daram.toml.
    if cwd.join("daram.toml").exists() {
        terminal::error("`daram.toml` already exists in this directory");
        return 1;
    }

    // Derive package name from directory name.
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    let name = sanitise_name(&dir_name);

    if let Err(e) = validate_package_name(&name) {
        terminal::error(&format!(
            "directory name `{}` is not a valid package name: {}",
            dir_name, e
        ));
        terminal::info(
            "Rename the directory or use `dr new <name>` to create a project with a specific name.",
        );
        return 1;
    }

    let manifest = Manifest {
        package: PackageMeta {
            name: name.clone(),
            version: "1.0.0".to_string(),
            edition: "2026".to_string(),
            description: None,
            authors: Vec::new(),
            license: Some("MIT".to_string()),
            repository: None,
            readme: None,
            keywords: Vec::new(),
        },
        build: BuildConfig::default(),
        dependencies: Default::default(),
        dev_dependencies: Default::default(),
        features: Default::default(),
        fmt: FmtConfig::default(),
        lint: LintConfig::default(),
        doc: DocConfig::default(),
    };

    // Create src/ and default source file if they don't exist.
    if !cwd.join("src").exists() {
        if let Err(e) = create_project_skeleton(&cwd, &name, false) {
            terminal::error(&format!("failed to create project skeleton: {}", e));
            return 1;
        }
    }

    if let Err(e) = manifest.write_to_dir(&cwd) {
        terminal::error(&format!("failed to write daram.toml: {}", e));
        return 1;
    }

    terminal::success(&format!("initialised `{}` in current directory", name));
    0
}

/// Convert a directory name to a valid package name by replacing invalid
/// characters with hyphens and lowercasing.
fn sanitise_name(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
