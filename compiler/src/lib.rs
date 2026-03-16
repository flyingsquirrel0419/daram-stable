//! # Daram Compiler
//!
//! Bootstrap compiler for the Daram programming language.
//!
//! ## Pipeline
//! ```text
//! Source → Lexer → Parser → AST → HIR → NameResolution → TypeChecker → MIR → Backend
//! ```
//!
//! ## Design principles
//! - TypeScript-style surface syntax
//! - Rust-style static semantics and ownership
//! - OCaml/ReScript-style module system and pattern matching
//! - Move-style abilities and capabilities permission model
//! - Zig-style `errdefer` cleanup
//! - Go/Elm-style minimal, consistent tooling

pub mod ast;
pub mod backend_capabilities;
pub mod builtin_catalog;
pub mod c_backend;
pub mod cranelift_backend;
pub mod diagnostics;
pub mod hir;
pub mod interpreter;
pub mod lexer;
mod lib_frontend;
pub mod mir;
pub mod monomorphize;
pub mod name_resolution;
pub mod native_runtime;
pub mod parser;
pub mod session;
pub mod source;
pub mod stdlib_bundle;
pub mod type_checker;
mod type_checker_abilities;
mod type_checker_borrow;
mod type_checker_match;
mod type_checker_places;
mod type_checker_prepare;

pub use session::Session;
pub use source::{SourceFile, SourceMap};
use std::collections::{BTreeMap, HashSet};

const PANIC_RECOVERY_MESSAGE: &str = "compiler aborted while handling input";

#[derive(Default)]
struct AstBundleNode {
    file: Option<source::FileId>,
    items: Option<Vec<ast::Item>>,
    children: BTreeMap<String, AstBundleNode>,
    child_order: Vec<String>,
}

pub struct AnalysisResult {
    pub session: Session,
    pub ast: Option<ast::Module>,
    pub hir: Option<hir::HirModule>,
    pub diagnostics: Vec<diagnostics::Diagnostic>,
}

pub struct MirAnalysisResult {
    pub session: Session,
    pub ast: Option<ast::Module>,
    pub hir: Option<hir::HirModule>,
    pub mir: Option<mir::MirModule>,
    pub diagnostics: Vec<diagnostics::Diagnostic>,
}

pub fn analyze(source: &str, file_name: &str) -> AnalysisResult {
    lib_frontend::analyze_source(source, file_name)
}

pub fn lower_to_codegen_mir(
    hir: &hir::HirModule,
) -> (mir::MirModule, Vec<diagnostics::Diagnostic>) {
    let (mir_module, diagnostics) = mir::lower(hir);
    if diagnostics
        .iter()
        .any(|diag| diag.level == diagnostics::Level::Error)
    {
        return (mir_module, diagnostics);
    }
    (monomorphize::monomorphize(&mir_module, hir), diagnostics)
}

pub fn analyze_to_codegen_mir(source: &str, file_name: &str) -> MirAnalysisResult {
    let analyzed = analyze(source, file_name);
    let AnalysisResult {
        session,
        ast,
        hir,
        diagnostics,
    } = analyzed;
    let mut diagnostics = diagnostics;

    if diagnostics
        .iter()
        .any(|diag| diag.level == diagnostics::Level::Error)
    {
        return MirAnalysisResult {
            session,
            ast,
            hir,
            mir: None,
            diagnostics,
        };
    }

    let Some(hir) = hir else {
        return MirAnalysisResult {
            session,
            ast,
            hir: None,
            mir: None,
            diagnostics,
        };
    };

    let (mir, mir_diagnostics) = lower_to_codegen_mir(&hir);
    diagnostics.extend(mir_diagnostics);
    let mir = if diagnostics
        .iter()
        .any(|diag| diag.level == diagnostics::Level::Error)
    {
        None
    } else {
        Some(mir)
    };

    MirAnalysisResult {
        session,
        ast,
        hir: Some(hir),
        mir,
        diagnostics,
    }
}

/// Compile a source string and return the result or diagnostics.
pub fn compile(source: &str, file_name: &str) -> CompileResult {
    let analyzed = analyze(source, file_name);
    let AnalysisResult {
        session,
        ast,
        diagnostics,
        ..
    } = analyzed;
    CompileResult {
        session,
        ast,
        diagnostics,
    }
}

pub struct CompileResult {
    pub session: Session,
    pub ast: Option<ast::Module>,
    pub diagnostics: Vec<diagnostics::Diagnostic>,
}

impl CompileResult {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.level == diagnostics::Level::Error)
    }
}

#[cfg(test)]
mod tests {
    use super::compile;

    #[test]
    fn rejects_extreme_nesting_without_panicking() {
        let mut src = String::new();
        for _ in 0..600 {
            src.push('(');
        }
        src.push('0');
        for _ in 0..600 {
            src.push(')');
        }
        let result = compile(&src, "deep.dr");
        assert!(result.has_errors());
    }

    #[test]
    fn rejects_huge_input_without_panicking() {
        let src = "let a = 1;\n".repeat(250_000);
        let result = compile(&src, "huge.dr");
        assert!(result.has_errors());
    }
}
