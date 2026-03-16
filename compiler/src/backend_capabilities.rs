use crate::diagnostics::Diagnostic;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Interpreter,
    C,
    Cranelift,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendFeature {
    UnwindCalls,
    ErrdeferUnwind,
    IndirectCalls,
    PointerOffset,
    AggregateReturn,
    DropSemantics,
}

pub fn supports(backend: BackendKind, feature: BackendFeature) -> bool {
    match (backend, feature) {
        (BackendKind::Interpreter, BackendFeature::DropSemantics) => false,
        (BackendKind::Interpreter, _) => true,
        (BackendKind::C, BackendFeature::ErrdeferUnwind) => true,
        (BackendKind::C, BackendFeature::UnwindCalls) => true,
        (BackendKind::C, BackendFeature::IndirectCalls) => true,
        (BackendKind::C, BackendFeature::PointerOffset) => false,
        (BackendKind::C, BackendFeature::AggregateReturn) => true,
        (BackendKind::C, BackendFeature::DropSemantics) => false,
        (BackendKind::Cranelift, BackendFeature::UnwindCalls) => false,
        (BackendKind::Cranelift, BackendFeature::ErrdeferUnwind) => false,
        (BackendKind::Cranelift, BackendFeature::IndirectCalls) => false,
        (BackendKind::Cranelift, BackendFeature::PointerOffset) => false,
        (BackendKind::Cranelift, BackendFeature::AggregateReturn) => true,
        (BackendKind::Cranelift, BackendFeature::DropSemantics) => false,
    }
}

pub fn unsupported_feature_diagnostic(
    backend: BackendKind,
    feature: BackendFeature,
    detail: Option<String>,
) -> Diagnostic {
    let backend_name = match backend {
        BackendKind::Interpreter => "Interpreter",
        BackendKind::C => "C backend",
        BackendKind::Cranelift => "Cranelift backend",
    };
    let feature_name = match feature {
        BackendFeature::UnwindCalls => "unwind calls",
        BackendFeature::ErrdeferUnwind => "errdefer unwind",
        BackendFeature::IndirectCalls => "indirect calls",
        BackendFeature::PointerOffset => "pointer offset",
        BackendFeature::AggregateReturn => "aggregate return",
        BackendFeature::DropSemantics => "drop semantics",
    };
    let mut diagnostic =
        Diagnostic::error(format!("{backend_name} does not support {feature_name}"));
    if let Some(detail) = detail {
        diagnostic = diagnostic.with_note(detail);
    }
    diagnostic
}
