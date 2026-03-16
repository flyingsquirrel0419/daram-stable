use crate::{dependency_cache, terminal};
use daram_compiler::{
    analyze_to_codegen_mir,
    diagnostics::{Level, Renderer},
    interpreter::{self, Value},
};
use std::{fs, path::Path};

pub fn execute_source_function(path: &Path, function_name: &str) -> Result<Value, String> {
    execute_source_function_with_options(path, function_name, false)
}

pub fn execute_source_function_with_options(
    path: &Path,
    function_name: &str,
    include_dev_dependencies: bool,
) -> Result<Value, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read `{}`: {}", path.display(), error))?;
    let workspace_root = path
        .parent()
        .and_then(|parent| crate::workspace::find_workspace_from(parent).ok())
        .map(|workspace| workspace.root);
    let bundled = match workspace_root {
        Some(root) => dependency_cache::compose_workspace_source(
            &root,
            Some(path),
            &source,
            include_dev_dependencies,
        )?,
        None => daram_compiler::stdlib_bundle::with_bundled_prelude(
            &dependency_cache::strip_outer_attributes(&source),
        ),
    };
    let file_name = path.to_string_lossy().to_string();
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
