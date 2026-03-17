//! `dr run [-- args…]` — build and run the current project.

use crate::{
    commands::{build, exec_mir, init},
    terminal,
    workspace::{find_workspace, WorkspaceError},
};
use daram_compiler::interpreter::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

pub fn run(args: &[String]) -> i32 {
    // Split args at `--` to separate dr flags from program args.
    let (dr_args, prog_args): (Vec<String>, Vec<String>) = {
        let sep = args.iter().position(|a| a == "--");
        match sep {
            Some(pos) => (args[..pos].to_vec(), args[pos + 1..].to_vec()),
            None => (args.to_vec(), Vec::new()),
        }
    };

    if let Some(source_file) = source_file_arg(&dr_args) {
        return run_source_file(&source_file, &prog_args);
    }

    match find_workspace() {
        Ok(_) => {}
        Err(WorkspaceError::NotFound) => {
            if let Err(message) = auto_initialise_current_directory() {
                terminal::error(&message);
                return 1;
            }
        }
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    }

    // Build first.
    let exit = build::run(&dr_args);
    if exit != 0 {
        return exit;
    }

    let ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };

    let release = dr_args.iter().any(|a| a == "--release");
    let target_dir = ws
        .root
        .join("target")
        .join(if release { "release" } else { "debug" });
    let binary_name = format!(
        "{}{}",
        ws.manifest.package.name,
        if cfg!(windows) { ".exe" } else { "" }
    );
    let binary = target_dir.join(&binary_name);

    if !binary.exists() {
        terminal::error(
            "build artefact not found — native backend not yet implemented in this pre-release",
        );
        return 1;
    }

    terminal::step(&format!("running `{}`", binary.display()));

    let status = Command::new(&binary).args(&prog_args).status();

    match status {
        Ok(s) => s.code().unwrap_or(1),
        Err(e) => {
            terminal::error(&format!("failed to run binary: {}", e));
            1
        }
    }
}

fn source_file_arg(args: &[String]) -> Option<PathBuf> {
    if args.len() != 1 {
        return None;
    }

    let candidate = Path::new(&args[0]);
    if candidate.extension().and_then(|ext| ext.to_str()) != Some("dr") {
        return None;
    }

    Some(candidate.to_path_buf())
}

fn run_source_file(path: &Path, prog_args: &[String]) -> i32 {
    if !prog_args.is_empty() {
        terminal::error("program arguments are not supported for `dr run <file>.dr` yet");
        return 1;
    }

    terminal::step(&format!("running `{}`", path.display()));

    match exec_mir::execute_source_function(path, "main") {
        Ok(Value::Unit) | Ok(_) => 0,
        Err(message) => {
            terminal::error(&message);
            1
        }
    }
}

fn auto_initialise_current_directory() -> Result<(), String> {
    let outcome = init::ensure_current_dir_project()?;

    if outcome.used_fallback_name {
        terminal::info(&format!(
            "current directory name cannot be used as a package name; using `{}` in daram.toml",
            outcome.name
        ));
    }
    if outcome.created_manifest || outcome.created_main_source {
        terminal::success(&format!(
            "auto-initialised `{}` for `dr run`",
            outcome.name
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::source_file_arg;
    use std::path::PathBuf;

    #[test]
    fn detects_single_source_file_argument() {
        assert_eq!(
            source_file_arg(&["example.dr".to_string()]),
            Some(PathBuf::from("example.dr"))
        );
    }

    #[test]
    fn ignores_non_source_arguments() {
        assert_eq!(source_file_arg(&["--release".to_string()]), None);
        assert_eq!(
            source_file_arg(&["example.dr".to_string(), "--release".to_string()]),
            None
        );
    }
}
