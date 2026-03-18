//! Shared project initialisation helpers.

use std::env;

use crate::{
    manifest::{
        validate_package_name, BuildConfig, DocConfig, FmtConfig, LintConfig, Manifest, PackageMeta,
    },
    terminal,
    workspace::ensure_runnable_project_skeleton,
};

const FALLBACK_PACKAGE_NAME: &str = "app";

pub struct InitOutcome {
    pub name: String,
    pub used_fallback_name: bool,
    pub created_manifest: bool,
    pub created_main_source: bool,
}

pub fn run(args: &[String]) -> i32 {
    let _ = args;
    terminal::error("`dr init` was removed");
    terminal::info("Run `dr run` in the current directory to auto-initialise and execute.");
    1
}

pub fn ensure_current_dir_project() -> Result<InitOutcome, String> {
    let cwd = match env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            return Err(format!("cannot determine current directory: {}", e));
        }
    };

    // Derive package name from directory name.
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project")
        .to_string();

    let (name, used_fallback_name) = derive_package_name(&dir_name);

    let manifest_path = cwd.join("daram.toml");
    let main_source_path = cwd.join("src").join("main.dr");
    let created_manifest = !manifest_path.exists();
    let created_main_source = !main_source_path.exists();
    let manifest = Manifest {
        package: PackageMeta {
            name: name.clone(),
            version: "1.0.2".to_string(),
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

    if let Err(e) = ensure_runnable_project_skeleton(&cwd, &name) {
        return Err(format!("failed to create project skeleton: {}", e));
    }

    if created_manifest {
        if let Err(e) = manifest.write_to_dir(&cwd) {
            return Err(format!("failed to write daram.toml: {}", e));
        }
    }

    Ok(InitOutcome {
        name,
        used_fallback_name,
        created_manifest,
        created_main_source,
    })
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
