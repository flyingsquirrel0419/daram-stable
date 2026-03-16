//! `dr new <name>` — create a new Daram project.

use std::{env, fs, path::PathBuf};

use crate::{
    manifest::{
        validate_package_name, BuildConfig, DocConfig, FmtConfig, LintConfig, Manifest, PackageMeta,
    },
    terminal,
    workspace::create_project_skeleton,
};

pub fn run(args: &[String]) -> i32 {
    let name = match args.first() {
        Some(n) => n.trim().to_string(),
        None => {
            terminal::error("usage: dr new <name>");
            return 1;
        }
    };

    // Validate the package name before creating anything.
    if let Err(e) = validate_package_name(&name) {
        terminal::error(&format!("invalid project name: {}", e));
        return 1;
    }

    let cwd = match env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            terminal::error(&format!("cannot determine current directory: {}", e));
            return 1;
        }
    };

    let project_dir = cwd.join(&name);

    // Refuse to clobber an existing directory unless it is empty.
    if project_dir.exists() {
        if !is_empty_dir(&project_dir) {
            terminal::error(&format!(
                "destination `{}` already exists and is not empty",
                project_dir.display()
            ));
            return 1;
        }
    }

    // Build the manifest.
    let lib = args.contains(&"--lib".to_string());
    let manifest = Manifest {
        package: PackageMeta {
            name: name.clone(),
            version: "1.0.0".to_string(),
            edition: "2026".to_string(),
            description: None,
            authors: Vec::new(),
            license: Some("MIT".to_string()),
            repository: None,
            readme: Some("README.md".to_string()),
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

    // Create directory structure and source files.
    if let Err(e) = create_project_skeleton(&project_dir, &name, lib) {
        terminal::error(&format!("failed to create project skeleton: {}", e));
        return 1;
    }

    // Write daram.toml.
    if let Err(e) = manifest.write_to_dir(&project_dir) {
        terminal::error(&format!("failed to write daram.toml: {}", e));
        return 1;
    }

    terminal::success(&format!(
        "created `{}` at `{}`",
        name,
        project_dir.display()
    ));
    terminal::info(&format!(
        "Enter the project: cd {}\nBuild:            dr build\nRun:              dr run",
        name
    ));
    0
}

fn is_empty_dir(path: &PathBuf) -> bool {
    fs::read_dir(path)
        .map(|mut d| d.next().is_none())
        .unwrap_or(false)
}
