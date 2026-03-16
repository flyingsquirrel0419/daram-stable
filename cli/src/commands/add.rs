//! `dr add <pkg>[@version]` — add a dependency to `daram.toml`.

use crate::{manifest::Dependency, terminal, workspace::find_workspace};

pub fn run(args: &[String]) -> i32 {
    let spec = match args.first() {
        Some(s) => s.as_str(),
        None => {
            terminal::error("usage: dr add <package>[@version] [--dev]");
            return 1;
        }
    };

    let dev = args.iter().any(|a| a == "--dev");

    // Parse `name@version` or just `name`.
    let (name, version) = match spec.split_once('@') {
        Some((n, v)) => (n.to_string(), v.to_string()),
        None => (spec.to_string(), "*".to_string()),
    };

    if name.is_empty() {
        terminal::error("package name must not be empty");
        return 1;
    }

    let mut ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };

    let dep = Dependency::version_only(&version);

    if dev {
        ws.manifest.dev_dependencies.insert(name.clone(), dep);
    } else {
        ws.manifest.dependencies.insert(name.clone(), dep);
    }

    if let Err(e) = ws.manifest.write_to_dir(&ws.root) {
        terminal::error(&format!("failed to update daram.toml: {}", e));
        return 1;
    }

    let kind = if dev { "dev-dependency" } else { "dependency" };
    terminal::success(&format!("added {} `{}@{}`", kind, name, version));
    terminal::info("Run `dr install` to fetch and install dependencies.");
    0
}
