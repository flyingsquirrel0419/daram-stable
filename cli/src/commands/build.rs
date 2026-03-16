//! `dr build` — compile the current project.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use crate::{dependency_cache, terminal, workspace::find_workspace};
use daram_compiler::{
    analyze_to_codegen_mir, c_backend, compile, cranelift_backend,
    diagnostics::{Level, Renderer},
    Session,
};

pub fn run(args: &[String]) -> i32 {
    let release = args.iter().any(|a| a == "--release");
    let check_only = args.iter().any(|a| a == "--check");

    let ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };

    let target = if release {
        crate::manifest::BuildTarget::NativeRelease
    } else {
        ws.manifest.build.target.clone()
    };

    terminal::step(&format!(
        "compiling `{}` v{} [{}]",
        ws.manifest.package.name,
        ws.manifest.package.version,
        target.as_str(),
    ));

    let src_dir = ws.root.join("src");
    if !src_dir.exists() {
        terminal::error("`src/` directory not found");
        return 1;
    }

    let start = Instant::now();
    let mut error_count = 0usize;
    let mut file_count = 0usize;

    if let Err(e) = compile_dir(&ws.root, &src_dir, &mut error_count, &mut file_count) {
        terminal::error(&format!("failed to enumerate source files: {}", e));
        return 1;
    }

    let elapsed = start.elapsed();

    if error_count > 0 {
        terminal::error(&format!(
            "build failed: {} error(s) in {} source file(s) ({:.2}s)",
            error_count,
            file_count,
            elapsed.as_secs_f64(),
        ));
        return 1;
    }

    if check_only {
        terminal::success(&format!(
            "check succeeded: {} file(s) ({:.2}s)",
            file_count,
            elapsed.as_secs_f64(),
        ));
        return 0;
    }

    let target_dir = ws
        .root
        .join("target")
        .join(if release { "release" } else { "debug" });
    if let Err(e) = fs::create_dir_all(&target_dir) {
        terminal::error(&format!("failed to create target directory: {}", e));
        return 1;
    }

    let entry_source = match find_entry_source(&src_dir) {
        Some(path) => path,
        None => {
            terminal::error("could not determine entry source file");
            return 1;
        }
    };

    let binary_name = format!(
        "{}{}",
        ws.manifest.package.name,
        if cfg!(windows) { ".exe" } else { "" }
    );
    let binary_path = target_dir.join(&binary_name);
    let native_target = matches!(
        target,
        crate::manifest::BuildTarget::NativeDebug | crate::manifest::BuildTarget::NativeRelease
    );
    if native_target {
        if let Err(e) = build_native_artifact(&ws.root, &entry_source, &binary_path, release) {
            terminal::error(&format!("failed to build native artifact: {}", e));
            return 1;
        }
    } else {
        let dr_exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(e) => {
                terminal::error(&format!("failed to locate current executable: {}", e));
                return 1;
            }
        };
        let script = if cfg!(windows) {
            format!(
                "@echo off\r\n\"{}\" __exec-mir \"{}\" main %*\r\n",
                dr_exe.display(),
                entry_source.display(),
            )
        } else {
            format!(
                "#!/usr/bin/env bash\nexec \"{}\" __exec-mir \"{}\" main \"$@\"\n",
                dr_exe.display(),
                entry_source.display(),
            )
        };

        if let Err(e) = fs::write(&binary_path, script) {
            terminal::error(&format!("failed to write build artifact: {}", e));
            return 1;
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o755)) {
                terminal::error(&format!("failed to mark build artifact executable: {}", e));
                return 1;
            }
        }
    }

    terminal::success(&format!(
        "built `{}` → `{}` ({:.2}s)",
        ws.manifest.package.name,
        binary_path.display(),
        elapsed.as_secs_f64(),
    ));
    0
}

fn find_entry_source(src_dir: &Path) -> Option<PathBuf> {
    let main = src_dir.join("main.dr");
    if main.is_file() {
        return Some(main);
    }
    find_first_dr_file(src_dir)
}

fn find_first_dr_file(dir: &Path) -> Option<PathBuf> {
    let mut entries = fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    entries.sort();

    for path in entries {
        if path.is_dir() {
            if let Some(found) = find_first_dr_file(&path) {
                return Some(found);
            }
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("dr") {
            return Some(path);
        }
    }

    None
}

fn compile_dir(
    workspace_root: &Path,
    dir: &PathBuf,
    error_count: &mut usize,
    file_count: &mut usize,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            compile_dir(workspace_root, &path, error_count, file_count)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("dr") {
            *file_count += 1;
            let src = fs::read_to_string(&path)?;
            let src = dependency_cache::compose_workspace_source(
                workspace_root,
                Some(&path),
                &src,
                false,
            )
            .map_err(std::io::Error::other)?;
            let name = path.to_string_lossy().to_string();
            let result = compile(&src, &name);
            if result.has_errors() {
                let renderer = Renderer::new(&result.session.source_map, true);
                for diag in &result.diagnostics {
                    eprint!("{}", renderer.render(diag));
                }
                *error_count += result
                    .diagnostics
                    .iter()
                    .filter(|d| d.level == daram_compiler::diagnostics::Level::Error)
                    .count();
            }
        }
    }
    Ok(())
}

fn build_native_artifact(
    workspace_root: &Path,
    entry_source: &Path,
    output_path: &Path,
    release: bool,
) -> Result<(), String> {
    let source = fs::read_to_string(entry_source)
        .map_err(|error| format!("failed to read `{}`: {}", entry_source.display(), error))?;
    let source = dependency_cache::compose_workspace_source(
        workspace_root,
        Some(entry_source),
        &source,
        false,
    )?;
    let (hir, mir_module) = compile_entry_to_mir(entry_source, &source)?;
    let mir_module = prune_to_reachable_defs(&hir, &mir_module, "main");
    let staging_dir = output_path
        .parent()
        .ok_or_else(|| "invalid output path".to_string())?
        .join(".native-build");
    fs::create_dir_all(&staging_dir)
        .map_err(|error| format!("failed to create native build directory: {}", error))?;

    match cranelift_backend::generate_object(&hir, &mir_module) {
        Ok(object) => {
            let object_path = staging_dir.join("main.o");
            fs::write(&object_path, object)
                .map_err(|error| format!("failed to write generated object file: {}", error))?;
            link_native_artifact(&[object_path], &staging_dir, output_path, release)?;
        }
        Err(cranelift_diagnostics) => {
            terminal::warn(&format!(
                "Cranelift backend fallback to C backend: {}",
                summarize_backend_diagnostics(&cranelift_diagnostics)
            ));
            let c_source = c_backend::generate_c(&hir, &mir_module).map_err(|c_diagnostics| {
                format!(
                    "Cranelift backend diagnostics:\n{}\n\nC backend diagnostics:\n{}",
                    render_backend_diagnostics(&cranelift_diagnostics),
                    render_backend_diagnostics(&c_diagnostics),
                )
            })?;
            let c_path = staging_dir.join("main.c");
            fs::write(&c_path, c_source)
                .map_err(|error| format!("failed to write generated C source: {}", error))?;
            link_native_artifact(&[c_path], &staging_dir, output_path, release)?;
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(output_path, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("failed to mark native artifact executable: {}", error))?;
    }

    Ok(())
}

fn prune_to_reachable_defs(
    hir: &daram_compiler::hir::HirModule,
    mir_module: &daram_compiler::mir::MirModule,
    entry_name: &str,
) -> daram_compiler::mir::MirModule {
    use daram_compiler::mir::{
        AggregateKind, MirConstItem, MirFn, Operand, Rvalue, StatementKind, TerminatorKind,
    };

    let function_map = mir_module
        .functions
        .iter()
        .map(|function| (function.def, function.clone()))
        .collect::<HashMap<_, _>>();
    let const_map = mir_module
        .consts
        .iter()
        .map(|item| (item.def, item.clone()))
        .collect::<HashMap<_, _>>();

    let Some(entry_def) = mir_module
        .functions
        .iter()
        .find(|function| {
            hir.def_names
                .get(&function.def)
                .is_some_and(|name| name == entry_name)
        })
        .map(|function| function.def)
    else {
        return mir_module.clone();
    };

    fn operand_defs(operand: &Operand, out: &mut Vec<daram_compiler::hir::DefId>) {
        if let Operand::Def(def) = operand {
            out.push(*def);
        }
    }

    fn rvalue_defs(rvalue: &Rvalue, out: &mut Vec<daram_compiler::hir::DefId>) {
        match rvalue {
            Rvalue::Use(operand) | Rvalue::Cast { operand, .. } => operand_defs(operand, out),
            Rvalue::BinaryOp { lhs, rhs, .. } => {
                operand_defs(lhs, out);
                operand_defs(rhs, out);
            }
            Rvalue::UnaryOp { operand, .. } => operand_defs(operand, out),
            Rvalue::Aggregate(kind, operands) => {
                if let AggregateKind::Closure(def) = kind {
                    out.push(*def);
                }
                for operand in operands {
                    operand_defs(operand, out);
                }
            }
            Rvalue::Read(_)
            | Rvalue::Ref { .. }
            | Rvalue::AddressOf { .. }
            | Rvalue::Discriminant(_)
            | Rvalue::Len(_) => {}
        }
    }

    fn function_defs(function: &MirFn) -> Vec<daram_compiler::hir::DefId> {
        let mut defs = Vec::new();
        for block in &function.basic_blocks {
            for statement in &block.statements {
                if let StatementKind::Assign(_, rvalue) = &statement.kind {
                    rvalue_defs(rvalue, &mut defs);
                }
            }
            let Some(terminator) = &block.terminator else {
                continue;
            };
            match &terminator.kind {
                TerminatorKind::Call { callee, args, .. } => {
                    operand_defs(callee, &mut defs);
                    for arg in args {
                        operand_defs(arg, &mut defs);
                    }
                }
                TerminatorKind::SwitchInt { discriminant, .. }
                | TerminatorKind::Assert {
                    cond: discriminant, ..
                } => operand_defs(discriminant, &mut defs),
                TerminatorKind::Unreachable
                | TerminatorKind::Goto(_)
                | TerminatorKind::Return
                | TerminatorKind::ErrdeferUnwind(_)
                | TerminatorKind::Drop { .. } => {}
            }
        }
        defs
    }

    let mut reachable_functions = HashSet::new();
    let mut reachable_consts = HashSet::new();
    let mut queue = VecDeque::from([entry_def]);

    while let Some(def) = queue.pop_front() {
        if !reachable_functions.insert(def) {
            continue;
        }
        let Some(function) = function_map.get(&def) else {
            continue;
        };
        for referenced in function_defs(function) {
            if function_map.contains_key(&referenced) && !reachable_functions.contains(&referenced)
            {
                queue.push_back(referenced);
            }
            if const_map.contains_key(&referenced) {
                reachable_consts.insert(referenced);
            }
        }
    }

    let functions = mir_module
        .functions
        .iter()
        .filter(|function| reachable_functions.contains(&function.def))
        .cloned()
        .collect::<Vec<MirFn>>();
    let consts = mir_module
        .consts
        .iter()
        .filter(|item: &&MirConstItem| reachable_consts.contains(&item.def))
        .cloned()
        .collect::<Vec<MirConstItem>>();

    daram_compiler::mir::MirModule {
        consts,
        functions,
        enum_variant_indices: mir_module.enum_variant_indices.clone(),
        enum_variant_names: mir_module.enum_variant_names.clone(),
        struct_field_names: mir_module.struct_field_names.clone(),
        display_impls: mir_module.display_impls.clone(),
        def_names: mir_module.def_names.clone(),
    }
}

fn link_native_artifact(
    sources: &[PathBuf],
    staging_dir: &Path,
    output_path: &Path,
    release: bool,
) -> Result<(), String> {
    let cc =
        find_c_compiler().ok_or_else(|| "failed to find `cc`, `clang`, or `gcc`".to_string())?;
    let runtime_path = staging_dir.join("runtime.c");
    fs::write(
        &runtime_path,
        daram_compiler::native_runtime::link_runtime_source(),
    )
    .map_err(|error| format!("failed to write native runtime source: {}", error))?;

    let mut command = Command::new(&cc);
    for source in sources {
        command.arg(source);
    }
    command.arg(&runtime_path).arg("-o").arg(output_path);
    if release {
        command.arg("-O2");
    } else {
        command.arg("-O0").arg("-g");
    }

    let output = command
        .output()
        .map_err(|error| format!("failed to invoke native linker `{cc}`: {}", error))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.trim().to_string());
    }
    Ok(())
}

fn compile_entry_to_mir(
    entry_source: &Path,
    source: &str,
) -> Result<
    (
        daram_compiler::hir::HirModule,
        daram_compiler::mir::MirModule,
    ),
    String,
> {
    let file_name = entry_source.display().to_string();
    let lowered = analyze_to_codegen_mir(source, &file_name);
    if lowered
        .diagnostics
        .iter()
        .any(|diag| diag.level == Level::Error)
    {
        return Err(render_diagnostics(&lowered.session, &lowered.diagnostics));
    }
    let hir = lowered
        .hir
        .ok_or_else(|| "frontend analysis did not produce HIR".to_string())?;
    let mir_module = lowered
        .mir
        .ok_or_else(|| "frontend analysis did not produce MIR".to_string())?;

    Ok((hir, mir_module))
}

fn render_diagnostics(
    session: &Session,
    diagnostics: &[daram_compiler::diagnostics::Diagnostic],
) -> String {
    let renderer = Renderer::new(&session.source_map, true);
    diagnostics
        .iter()
        .map(|diag| renderer.render(diag))
        .collect::<Vec<_>>()
        .join("")
}

fn summarize_backend_diagnostics(
    diagnostics: &[daram_compiler::diagnostics::Diagnostic],
) -> String {
    diagnostics
        .iter()
        .take(2)
        .map(|diag| diag.message.as_str())
        .collect::<Vec<_>>()
        .join("; ")
}

fn render_backend_diagnostics(diagnostics: &[daram_compiler::diagnostics::Diagnostic]) -> String {
    diagnostics
        .iter()
        .map(|diag| diag.message.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

fn find_c_compiler() -> Option<&'static str> {
    ["cc", "clang", "gcc"]
        .into_iter()
        .find(|candidate| Command::new(candidate).arg("--version").output().is_ok())
}
