//! `dr init` — initialise the current directory as a Daram project.

use std::env;

use crate::{
    manifest::{
        validate_package_name, BuildConfig, DocConfig, FmtConfig, LintConfig, Manifest, PackageMeta,
    },
    terminal,
    workspace::create_project_skeleton,
};

const FALLBACK_PACKAGE_NAME: &str = "app";

pub fn run(args: &[String]) -> i32 {
    if !args.is_empty() {
        terminal::error("usage: dr init");
        terminal::info("`dr init` always initialises the current directory.");
        return 1;
    }

    let cwd = match env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            terminal::error(&format!("cannot determine current directory: {}", e));
            return 1;
        }
    };

    let is_reinitialising = cwd.join("daram.toml").exists()
        || cwd.join("src").join("main.dr").exists()
        || cwd.join("dr.lock").exists();

    // Derive package name from directory name.
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    let (name, used_fallback_name) = derive_package_name(&dir_name);
    if used_fallback_name {
        terminal::info(&format!(
            "directory name `{}` cannot be used as a package name; using `{}` in daram.toml",
            dir_name, name
        ));
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

    if let Err(e) = create_project_skeleton(&cwd, &name, false) {
        terminal::error(&format!("failed to create project skeleton: {}", e));
        return 1;
    }

    if let Err(e) = manifest.write_to_dir(&cwd) {
        terminal::error(&format!("failed to write daram.toml: {}", e));
        return 1;
    }

    let action = if is_reinitialising {
        "reinitialised"
    } else {
        "initialised"
    };
    terminal::success(&format!("{} `{}` in current directory", action, name));
    0
}

fn derive_package_name(dir_name: &str) -> (String, bool) {
    let sanitized = sanitise_name(dir_name);
    if validate_package_name(&sanitized).is_ok() {
        return (sanitized, false);
    }

    (FALLBACK_PACKAGE_NAME.to_string(), true)
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

#[cfg(test)]
mod tests {
    use super::derive_package_name;

    #[test]
    fn uses_sanitized_directory_name_when_valid() {
        let (name, used_fallback) = derive_package_name("Hello World");
        assert_eq!(name, "hello-world");
        assert!(!used_fallback);
    }

    #[test]
    fn falls_back_for_non_ascii_directory_names() {
        let (name, used_fallback) = derive_package_name("ㅇㄱ");
        assert_eq!(name, "app");
        assert!(used_fallback);
    }
}
