//! Workspace discovery: walks up the directory tree to find `daram.toml`.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use crate::manifest::{Manifest, ManifestError};

/// The result of locating and loading a workspace manifest.
pub struct Workspace {
    /// Absolute path to the directory containing `daram.toml`.
    pub root: PathBuf,
    pub manifest: Manifest,
}

#[derive(Debug)]
pub enum WorkspaceError {
    NotFound,
    Manifest(ManifestError),
    Io(io::Error),
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkspaceError::NotFound => write!(
                f,
                "could not find `daram.toml` in the current directory or any parent directory\n\
                 Hint: run `dr run` here to auto-initialise a runnable project, or `dr new <name>` to create one."
            ),
            WorkspaceError::Manifest(e) => write!(f, "error reading manifest: {}", e),
            WorkspaceError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl From<ManifestError> for WorkspaceError {
    fn from(e: ManifestError) -> Self {
        WorkspaceError::Manifest(e)
    }
}

impl From<io::Error> for WorkspaceError {
    fn from(e: io::Error) -> Self {
        WorkspaceError::Io(e)
    }
}

/// Search the current directory and all ancestors for `daram.toml`.
pub fn find_workspace() -> Result<Workspace, WorkspaceError> {
    let cwd = env::current_dir().map_err(WorkspaceError::Io)?;
    find_workspace_from(&cwd)
}

pub fn find_workspace_from(start: &Path) -> Result<Workspace, WorkspaceError> {
    let mut dir = start.to_path_buf();
    loop {
        let candidate = dir.join("daram.toml");
        if candidate.is_file() {
            let manifest = Manifest::from_file(&candidate)?;
            return Ok(Workspace {
                root: dir,
                manifest,
            });
        }
        if !dir.pop() {
            return Err(WorkspaceError::NotFound);
        }
    }
}

/// Create a standard project skeleton in `dir`.
/// If `lib` is `true`, creates a library; otherwise an executable.
pub fn create_project_skeleton(dir: &Path, name: &str, lib: bool) -> Result<(), io::Error> {
    fs::create_dir_all(dir)?;
    fs::create_dir_all(dir.join("src"))?;

    // Default source file
    if lib {
        fs::write(dir.join("src").join("lib.dr"), default_lib_source(name))?;
    } else {
        fs::write(dir.join("src").join("main.dr"), default_main_source(name))?;
    }

    // dr.lock (empty)
    fs::write(dir.join("dr.lock"), "")?;

    // .gitignore
    fs::write(dir.join(".gitignore"), "/target\n/dr.lock\n")?;

    // Initialise git repo if git is available
    let _ = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(dir)
        .status();

    Ok(())
}

/// Ensure the minimum runnable project scaffold exists without overwriting
/// existing user files.
pub fn ensure_runnable_project_skeleton(dir: &Path, name: &str) -> Result<(), io::Error> {
    fs::create_dir_all(dir)?;
    fs::create_dir_all(dir.join("src"))?;

    let main_path = dir.join("src").join("main.dr");
    if !main_path.exists() {
        fs::write(main_path, default_main_source(name))?;
    }

    let lock_path = dir.join("dr.lock");
    if !lock_path.exists() {
        fs::write(lock_path, "")?;
    }

    let gitignore_path = dir.join(".gitignore");
    if !gitignore_path.exists() {
        fs::write(gitignore_path, "/target\n/dr.lock\n")?;
    }

    Ok(())
}

fn default_main_source(name: &str) -> String {
    format!(
        r#"// {name} — a Daram program
//
// Run with: dr run

fun main() {{
}}
"#,
        name = name,
    )
}

fn default_lib_source(name: &str) -> String {
    format!(
        r#"// {name} — a Daram library
//
// This file is the root module of the `{name}` library.

/// Returns the provided name.
export fun greet(name: string): string {{
    name
}}
"#,
        name = name,
    )
}

#[cfg(test)]
mod tests {
    use super::default_main_source;

    #[test]
    fn default_main_source_prints_visible_output() {
        let source = default_main_source("demo");
        assert!(source.contains("fun main() {"));
        assert!(!source.contains("println("));
        assert!(!source.contains("0"));
    }
}
