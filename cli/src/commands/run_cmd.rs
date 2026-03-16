//! `dr run [-- args…]` — build and run the current project.

use crate::{commands::build, terminal, workspace::find_workspace};
use std::process::Command;

pub fn run(args: &[String]) -> i32 {
    // Split args at `--` to separate dr flags from program args.
    let (dr_args, prog_args): (Vec<String>, Vec<String>) = {
        let sep = args.iter().position(|a| a == "--");
        match sep {
            Some(pos) => (args[..pos].to_vec(), args[pos + 1..].to_vec()),
            None => (args.to_vec(), Vec::new()),
        }
    };

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
