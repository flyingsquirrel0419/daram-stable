//! `dr test [filter]` — compile and run the project's test suite.

use std::{fs, path::PathBuf, time::Instant};

use crate::{commands::exec_mir, terminal, workspace::find_workspace};

pub fn run(args: &[String]) -> i32 {
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let filter = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .map(String::as_str)
        .unwrap_or("");

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
        "testing `{}` v{}",
        ws.manifest.package.name, ws.manifest.package.version
    ));

    let start = Instant::now();

    let tests = match collect_tests(&src_dir, filter) {
        Ok(t) => t,
        Err(e) => {
            terminal::error(&format!("failed to collect tests: {}", e));
            return 1;
        }
    };

    if tests.is_empty() {
        terminal::info("no tests found");
        return 0;
    }

    let total = tests.len();
    let mut passed = 0usize;
    let mut failed = 0usize;

    for test in &tests {
        if verbose {
            terminal::step(&format!("  running `{}`…", test.qualified));
        }
        let result =
            exec_mir::execute_source_function_with_options(&test.path, &test.function_name, true);
        let ok = match (test.should_panic, result) {
            (false, Ok(_)) => true,
            (true, Err(_)) => true,
            (false, Err(message)) => {
                terminal::error(&format!("test `{}` failed: {}", test.qualified, message));
                false
            }
            (true, Ok(_)) => {
                terminal::error(&format!(
                    "test `{}` was expected to panic but completed successfully",
                    test.qualified
                ));
                false
            }
        };
        if ok {
            passed += 1;
            if verbose {
                terminal::success(&format!("  ok: `{}`", test.qualified));
            }
        } else {
            failed += 1;
            if verbose {
                terminal::error(&format!("  failed: `{}`", test.qualified));
            }
        }
    }

    let elapsed = start.elapsed();

    eprintln!();
    eprintln!(
        "test result: {} ({}/{} passed in {:.2}s)",
        if failed == 0 {
            "\x1b[32mok\x1b[0m"
        } else {
            "\x1b[31mFAILED\x1b[0m"
        },
        passed,
        total,
        elapsed.as_secs_f64(),
    );

    if failed > 0 {
        1
    } else {
        0
    }
}

#[derive(Debug, Clone)]
struct TestCase {
    path: PathBuf,
    qualified: String,
    function_name: String,
    should_panic: bool,
}

fn collect_tests(src_dir: &PathBuf, filter: &str) -> std::io::Result<Vec<TestCase>> {
    let mut tests = Vec::new();
    collect_tests_recursive(src_dir, src_dir, filter, &mut tests)?;
    Ok(tests)
}

fn collect_tests_recursive(
    src_root: &PathBuf,
    dir: &PathBuf,
    filter: &str,
    out: &mut Vec<TestCase>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_tests_recursive(src_root, &path, filter, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("dr") {
            let src = fs::read_to_string(&path)?;
            let lines = src.lines().collect::<Vec<_>>();
            let mut pending_test = false;
            let mut pending_should_panic = false;

            for line in lines {
                let trimmed = line.trim();
                match trimmed {
                    "#[test]" => pending_test = true,
                    "#[should_panic]" => pending_should_panic = true,
                    _ => {
                        if pending_test {
                            if let Some(fn_name) = extract_fn_name(trimmed) {
                                let entry_name =
                                    module_entry_name_for_path(src_root, &path, fn_name);
                                let qualified = entry_name.clone();
                                if filter.is_empty() || qualified.contains(filter) {
                                    out.push(TestCase {
                                        path: path.clone(),
                                        qualified,
                                        function_name: entry_name,
                                        should_panic: pending_should_panic,
                                    });
                                }
                                pending_test = false;
                                pending_should_panic = false;
                            } else if !trimmed.is_empty() && !trimmed.starts_with("#[") {
                                pending_test = false;
                                pending_should_panic = false;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn module_entry_name_for_path(src_root: &PathBuf, path: &PathBuf, fn_name: &str) -> String {
    let Ok(relative) = path.strip_prefix(src_root) else {
        return fn_name.to_string();
    };

    let mut segments = relative
        .iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return fn_name.to_string();
    }

    let last = segments.pop().unwrap_or_default();
    match last.as_str() {
        "main.dr" | "lib.dr" | "mod.dr" => {}
        other => segments.push(other.trim_end_matches(".dr").to_string()),
    }

    if segments.is_empty() {
        fn_name.to_string()
    } else {
        format!("{}::{}", segments.join("::"), fn_name)
    }
}

fn extract_fn_name(line: &str) -> Option<&str> {
    let line = line.trim();
    let line = if line.starts_with("pub ") {
        &line[4..].trim_start()
    } else if line.starts_with("export ") {
        &line[7..].trim_start()
    } else {
        line
    };
    let line = if line.starts_with("async ") {
        &line[6..]
    } else {
        line
    };
    let line = line
        .strip_prefix("fn ")
        .or_else(|| line.strip_prefix("fun "))?
        .trim_start();
    let name_end = line.find('(')?;
    let name = line[..name_end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::{collect_tests, extract_fn_name, module_entry_name_for_path};
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn unique_temp_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{}_{}_{}", name, std::process::id(), nanos))
    }

    #[test]
    fn extracts_legacy_and_new_surface_function_names() {
        assert_eq!(extract_fn_name("fn main() {"), Some("main"));
        assert_eq!(extract_fn_name("fun main() {"), Some("main"));
        assert_eq!(extract_fn_name("export fun helper() {"), Some("helper"));
        assert_eq!(extract_fn_name("pub async fn helper() {"), Some("helper"));
    }

    #[test]
    fn qualifies_test_entries_from_file_modules() {
        let src_root = PathBuf::from("/tmp/project/src");
        assert_eq!(
            module_entry_name_for_path(&src_root, &src_root.join("main.dr"), "smoke"),
            "smoke"
        );
        assert_eq!(
            module_entry_name_for_path(&src_root, &src_root.join("smoke_test.dr"), "smoke"),
            "smoke_test::smoke"
        );
        assert_eq!(
            module_entry_name_for_path(&src_root, &src_root.join("nested").join("mod.dr"), "smoke",),
            "nested::smoke"
        );
        assert_eq!(
            module_entry_name_for_path(
                &src_root,
                &src_root.join("nested").join("helpers.dr"),
                "smoke",
            ),
            "nested::helpers::smoke"
        );
    }

    #[test]
    fn collects_nested_tests_with_module_qualified_names() {
        let dir = unique_temp_dir("daram_collect_tests");
        let src_dir = dir.join("src").join("nested");
        fs::create_dir_all(&src_dir).expect("expected temp src dir creation");
        let file = src_dir.join("helpers.dr");
        fs::write(&file, "#[test]\nfun smoke() {}\n#[test]\nfun helper() {}\n")
            .expect("expected test fixture write");

        let tests = collect_tests(&dir.join("src"), "nested::helpers")
            .expect("expected test collection to succeed");
        assert_eq!(tests.len(), 2);
        assert_eq!(tests[0].qualified, "nested::helpers::smoke");
        assert_eq!(tests[0].function_name, "nested::helpers::smoke");
        assert_eq!(tests[1].qualified, "nested::helpers::helper");
        assert_eq!(tests[1].function_name, "nested::helpers::helper");

        fs::remove_dir_all(&dir).expect("expected temp dir cleanup");
    }
}
