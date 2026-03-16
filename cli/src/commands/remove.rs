//! `dr remove <pkg>` — remove a dependency from `daram.toml`.

use crate::{terminal, workspace::find_workspace};

pub fn run(args: &[String]) -> i32 {
    let name = match args.first() {
        Some(n) => n.as_str(),
        None => {
            terminal::error("usage: dr remove <package>");
            return 1;
        }
    };

    let mut ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };

    let removed_dep = ws.manifest.dependencies.remove(name).is_some();
    let removed_dev = ws.manifest.dev_dependencies.remove(name).is_some();

    if !removed_dep && !removed_dev {
        terminal::warn(&format!(
            "package `{}` was not listed as a dependency",
            name
        ));
        return 0;
    }

    if let Err(e) = ws.manifest.write_to_dir(&ws.root) {
        terminal::error(&format!("failed to update daram.toml: {}", e));
        return 1;
    }

    terminal::success(&format!("removed `{}`", name));
    0
}
