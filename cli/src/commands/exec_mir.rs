use crate::{dependency_cache, terminal};
use daram_compiler::{
    analyze_to_codegen_mir,
    diagnostics::{Level, Renderer},
    interpreter::{self, Value},
};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn execute_source_function(path: &Path, function_name: &str) -> Result<Value, String> {
    execute_source_function_with_options(path, function_name, false)
}

pub fn execute_source_function_with_options(
    path: &Path,
    function_name: &str,
    include_dev_dependencies: bool,
) -> Result<Value, String> {
    let canonical_path = canonicalize_source_path(path)?;
    let source = fs::read_to_string(&canonical_path)
        .map_err(|error| format!("failed to read `{}`: {}", canonical_path.display(), error))?;
    let workspace_root = canonical_path
        .parent()
        .and_then(|parent| crate::workspace::find_workspace_from(parent).ok())
        .map(|workspace| workspace.root);
    let (bundled, file_name) = match workspace_root {
        Some(root) => (
            dependency_cache::compose_workspace_source(
                &root,
                Some(&canonical_path),
                &source,
                include_dev_dependencies,
            )?,
            canonical_path.to_string_lossy().to_string(),
        ),
        None => (
            dependency_cache::compose_standalone_source(&canonical_path, &source)?,
            "main.dr".to_string(),
        ),
    };
    let lowered = analyze_to_codegen_mir(&bundled, &file_name);
    if !lowered
        .diagnostics
        .iter()
        .any(|diag| diag.level == Level::Error)
    {
        if let Some(mir_module) = lowered.mir.as_ref() {
            return interpreter::execute_function(
                &mir_module,
                &mir_module.def_names,
                function_name,
                &[],
            )
            .map_err(|error| error.message);
        }
    }

    let renderer = Renderer::new(&lowered.session.source_map, true);
    for diagnostic in &lowered.diagnostics {
        eprint!("{}", renderer.render(diagnostic));
    }
    Err(format!("failed to execute `{}`", function_name))
}

fn canonicalize_source_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    let cwd = std::env::current_dir()
        .map_err(|error| format!("failed to resolve current directory: {}", error))?;
    Ok(cwd.join(path))
}

pub fn run_internal(args: &[String]) -> i32 {
    if args.len() < 2 {
        terminal::error("usage: dr __exec-mir <source-file> <function>");
        return 1;
    }

    let source = std::path::Path::new(&args[0]);
    let function_name = &args[1];
    match execute_source_function(source, function_name) {
        Ok(Value::Int(code)) => i32::try_from(code).unwrap_or(0),
        Ok(Value::Uint(code)) => i32::try_from(code).unwrap_or(0),
        Ok(_) => 0,
        Err(message) => {
            terminal::error(&message);
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::execute_source_function;
    use daram_compiler::interpreter::Value;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn executes_standalone_file_with_relative_imports() {
        let root = temp_test_dir("exec-mir-standalone");
        let main_path = root.join("hello.dr");
        fs::write(
            &main_path,
            "import { answer } from \"./helper\";\nfun main(): i32 { answer() }\n",
        )
        .unwrap();
        fs::write(root.join("helper.dr"), "export fun answer(): i32 { 42 }\n").unwrap();

        let value = execute_source_function(&main_path, "main").unwrap();
        assert!(matches!(value, Value::Int(42)));
    }

    fn temp_test_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("dr-cli-{prefix}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
