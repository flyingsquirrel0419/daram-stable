//! `dr lint` — static analysis and lint checking.

use std::{fs, path::PathBuf};

use crate::{terminal, workspace::find_workspace};
use daram_compiler::{compile, diagnostics::Renderer, stdlib_bundle};

pub fn run(args: &[String]) -> i32 {
    let fix = args.iter().any(|a| a == "--fix");

    let ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };

    let src_dir = ws.root.join("src");
    if !src_dir.exists() {
        terminal::error("`src/` directory not found");
        return 1;
    }

    terminal::step(&format!(
        "linting `{}` v{}",
        ws.manifest.package.name, ws.manifest.package.version
    ));

    let mut warning_count = 0usize;
    let mut error_count = 0usize;

    if let Err(e) = lint_dir(&src_dir, fix, &mut warning_count, &mut error_count) {
        terminal::error(&format!("I/O error while linting: {}", e));
        return 1;
    }

    if error_count > 0 {
        terminal::error(&format!(
            "lint failed: {} error(s), {} warning(s)",
            error_count, warning_count
        ));
        return 1;
    }

    if warning_count > 0 {
        terminal::warn(&format!("lint passed with {} warning(s)", warning_count));
    } else {
        terminal::success("no lint issues found");
    }
    0
}

fn lint_dir(
    dir: &PathBuf,
    fix: bool,
    warnings: &mut usize,
    errors: &mut usize,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            lint_dir(&path, fix, warnings, errors)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("dr") {
            let src = fs::read_to_string(&path)?;
            let name = path.to_string_lossy().to_string();
            let result = compile(&src, &name);
            let renderer = Renderer::new(&result.session.source_map, true);
            for diag in &result.diagnostics {
                match diag.level {
                    daram_compiler::diagnostics::Level::Error => {
                        *errors += 1;
                        eprint!("{}", renderer.render(diag));
                    }
                    daram_compiler::diagnostics::Level::Warning => {
                        *warnings += 1;
                        eprint!("{}", renderer.render(diag));
                    }
                    _ => {}
                }
            }
            // Built-in lint checks.
            run_builtin_lints(&src, &name, warnings, errors);
        }
    }
    Ok(())
}

/// Simple text-based lint checks that don't require a full AST.
fn run_builtin_lints(src: &str, file: &str, warnings: &mut usize, errors: &mut usize) {
    run_binding_lints(src, file, warnings);

    let mut terminated_flow = false;
    for (line_no, line) in src.lines().enumerate() {
        let line_no = line_no + 1;
        let trimmed = line.trim();

        let unreachable = terminated_flow
            && !trimmed.is_empty()
            && trimmed != "}"
            && !trimmed.starts_with("//")
            && !trimmed.starts_with("#[");
        if unreachable {
            eprintln!(
                "warning: {}:{}: unreachable code after terminating statement",
                file, line_no
            );
            *warnings += 1;
        }

        // Warn on `unsafe` blocks outside of safety comments.
        if line.contains("unsafe {") && !has_safety_comment(src, line_no) {
            eprintln!(
                "warning: {}:{}: `unsafe` block without a `// SAFETY:` comment",
                file, line_no
            );
            *warnings += 1;
        }

        // Warn on lines longer than 120 characters.
        if line.len() > 120 {
            eprintln!(
                "warning: {}:{}: line is {} chars (max 120)",
                file,
                line_no,
                line.len()
            );
            *warnings += 1;
        }

        // Warn on `TODO` or `FIXME` comments.
        if line.contains("// TODO") {
            eprintln!("note: {}:{}: found TODO comment", file, line_no);
        }
        if line.contains("// FIXME") {
            eprintln!("note: {}:{}: found FIXME comment", file, line_no);
        }

        if trimmed.contains("=> {}") || trimmed.ends_with("=> {},") || trimmed.ends_with("=> {} ,")
        {
            eprintln!("warning: {}:{}: empty match arm", file, line_no);
            *warnings += 1;
        }

        if let Some(path) = trimmed
            .strip_prefix("use ")
            .and_then(|rest| rest.split_whitespace().next())
            .map(|path| path.trim_end_matches(';'))
        {
            if stdlib_bundle::is_unstable_std_path(path) {
                eprintln!("warning: {}:{}: unstable stdlib import", file, line_no);
                *warnings += 1;
            }
        }

        // Deny `eval` usage.
        if line.contains("eval(") {
            eprintln!(
                "error: {}:{}: `eval` is not allowed in Daram",
                file, line_no
            );
            *errors += 1;
        }

        terminated_flow = if unreachable {
            false
        } else {
            starts_terminating_flow(trimmed)
        };
    }
}

/// Check if the line before `line_no` (1-based) is a `// SAFETY:` comment.
fn has_safety_comment(src: &str, line_no: usize) -> bool {
    if line_no <= 1 {
        return false;
    }
    src.lines()
        .nth(line_no - 2)
        .map(|l| l.trim().starts_with("// SAFETY:"))
        .unwrap_or(false)
}

fn run_binding_lints(src: &str, file: &str, warnings: &mut usize) {
    let lines = src.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        let Some(binding) = extract_let_binding(line) else {
            continue;
        };

        let usage_count = count_identifier_occurrences(src, &binding.name);
        if usage_count <= 1 {
            eprintln!(
                "warning: {}:{}: unused variable `{}`",
                file,
                index + 1,
                binding.name
            );
            *warnings += 1;
        }

        if binding.mutable && !has_mutating_use(src, &binding.name) {
            eprintln!(
                "warning: {}:{}: needless `mut` on `{}`",
                file,
                index + 1,
                binding.name
            );
            *warnings += 1;
        }
    }
}

struct LetBinding {
    name: String,
    mutable: bool,
}

fn extract_let_binding(line: &str) -> Option<LetBinding> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("let ")?;
    if rest.starts_with('(') || rest.starts_with('{') || rest.starts_with('[') {
        return None;
    }

    let (mutable, rest) = if let Some(rest) = rest.strip_prefix("mut ") {
        (true, rest)
    } else {
        (false, rest)
    };

    let name = rest
        .chars()
        .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>();
    if name.is_empty() {
        return None;
    }

    Some(LetBinding { name, mutable })
}

fn count_identifier_occurrences(src: &str, ident: &str) -> usize {
    src.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|token| *token == ident)
        .count()
}

fn has_mutating_use(src: &str, ident: &str) -> bool {
    src.lines().any(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with("let ") {
            return false;
        }
        trimmed.contains(&format!("{ident} ="))
            || trimmed.contains(&format!("&mut {ident}"))
            || trimmed.contains(&format!("({ident} ="))
            || trimmed.contains(&format!("[{ident}] ="))
    })
}

fn starts_terminating_flow(trimmed: &str) -> bool {
    trimmed.starts_with("return")
        || trimmed.starts_with("break")
        || trimmed.starts_with("continue")
        || trimmed.starts_with("panic(")
}

#[cfg(test)]
mod tests {
    use super::run_builtin_lints;

    #[test]
    fn eval_usage_counts_as_lint_error() {
        let mut warnings = 0usize;
        let mut errors = 0usize;

        run_builtin_lints(
            "fun main(): i32 { eval(\"1 + 1\"); 0 }\n",
            "main.dr",
            &mut warnings,
            &mut errors,
        );

        assert_eq!(warnings, 0);
        assert_eq!(errors, 1);
    }
}
