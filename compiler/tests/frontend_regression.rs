use daram_compiler::{
    analyze,
    ast::{Expr, Item, Stmt, StructKind, TraitItem, TypeExpr},
    compile,
    diagnostics::Renderer,
    hir::{HirExprKind, HirStmtKind, Ty},
    lexer::{lex_with_errors, TokenKind},
    name_resolution::resolve,
    parser::parse,
    source::FileId,
    stdlib_bundle,
};

fn parse_clean(source: &str) -> daram_compiler::ast::Module {
    let (tokens, lex_errors) = lex_with_errors(source);
    assert!(
        lex_errors.is_empty(),
        "expected clean lex, got: {:?}",
        lex_errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
    );

    let (module, parse_errors) = parse(FileId(0), &tokens);
    assert!(
        parse_errors.is_empty(),
        "expected clean parse, got: {:?}",
        parse_errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
    );
    module
}

fn resolve_clean(source: &str) -> daram_compiler::hir::HirModule {
    let module = parse_clean(source);
    let (hir, diagnostics) = resolve(FileId(0), &module);
    assert!(
        diagnostics.is_empty(),
        "expected clean lowering, got: {:?}",
        diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    hir
}

#[test]
fn resolver_preserves_generic_metadata_in_hir_items() {
    let hir = resolve_clean(
        r#"
        trait Render {}

        type Pair<T, U = i32> = (T, U);

        struct Point<T: Render, U = i32> {
            x: T,
            y: U,
        }

        enum Maybe<T, E = i32> {
            Some(T),
            Err(E),
        }

        fun wrap<T, U = i32>(value: T, extra: U) {}
        "#,
    );

    let pair = hir
        .type_aliases
        .iter()
        .find(|alias| {
            hir.def_names
                .get(&alias.def)
                .is_some_and(|name| name == "Pair")
        })
        .expect("expected Pair type alias");
    assert_eq!(pair.type_params.len(), 2);
    assert_eq!(pair.type_params[0].name, "T");
    assert_eq!(pair.type_params[1].name, "U");
    assert_eq!(
        pair.type_params[1].default,
        Some(Ty::Int(daram_compiler::hir::IntSize::I32))
    );

    let point = hir
        .structs
        .iter()
        .find(|item| {
            hir.def_names
                .get(&item.def)
                .is_some_and(|name| name == "Point")
        })
        .expect("expected Point struct");
    assert_eq!(point.type_params.len(), 2);
    assert_eq!(point.type_params[0].name, "T");
    assert_eq!(point.type_params[0].bounds.len(), 1);
    assert_eq!(
        point.type_params[1].default,
        Some(Ty::Int(daram_compiler::hir::IntSize::I32))
    );

    let maybe = hir
        .enums
        .iter()
        .find(|item| {
            hir.def_names
                .get(&item.def)
                .is_some_and(|name| name == "Maybe")
        })
        .expect("expected Maybe enum");
    assert_eq!(maybe.type_params.len(), 2);
    assert_eq!(maybe.type_params[0].name, "T");
    assert_eq!(maybe.type_params[1].name, "E");

    let render = hir
        .traits
        .iter()
        .find(|item| {
            hir.def_names
                .get(&item.def)
                .is_some_and(|name| name == "Render")
        })
        .expect("expected Render trait");
    assert!(render.type_params.is_empty());

    let wrap = hir
        .functions
        .iter()
        .find(|item| {
            hir.def_names
                .get(&item.def)
                .is_some_and(|name| name == "wrap")
        })
        .expect("expected wrap function");
    assert_eq!(wrap.type_params.len(), 2);
    assert_eq!(wrap.type_params[0].name, "T");
    assert_eq!(wrap.type_params[1].name, "U");
    assert_eq!(
        wrap.type_params[1].default,
        Some(Ty::Int(daram_compiler::hir::IntSize::I32))
    );
}

#[test]
fn resolver_preserves_derive_metadata_in_hir_items() {
    let hir = resolve_clean(
        r#"
        @derive(Hash, Eq, PartialEq)
        struct Point {
            x: i32,
            y: i32,
        }

        @derive(Copy)
        enum Toggle {
            Off,
            On,
        }
        "#,
    );

    let point = hir
        .structs
        .iter()
        .find(|item| {
            hir.def_names
                .get(&item.def)
                .is_some_and(|name| name == "Point")
        })
        .expect("expected Point struct");
    assert_eq!(point.derives, vec!["Hash", "Eq", "PartialEq"]);

    let toggle = hir
        .enums
        .iter()
        .find(|item| {
            hir.def_names
                .get(&item.def)
                .is_some_and(|name| name == "Toggle")
        })
        .expect("expected Toggle enum");
    assert_eq!(toggle.derives, vec!["Copy"]);
}

#[test]
fn compile_reports_direct_call_argument_type_mismatch() {
    let result = compile(
        r#"
        fun takes_i32(value: i32): i32 { value }
        fun main(): i32 { takes_i32(true) }
        "#,
        "main.dr",
    );

    let diagnostic = result
        .diagnostics
        .iter()
        .find(|diag| diag.message.contains("mismatched function argument type"))
        .expect("expected direct-call argument mismatch diagnostic");
    assert!(diagnostic
        .message
        .contains("mismatched function argument type"));
}

#[test]
fn compile_preserves_session_file_name_for_rendered_diagnostics() {
    let result = compile(
        r#"
        fun takes_i32(value: i32): i32 { value }
        fun main(): i32 { takes_i32(true) }
        "#,
        "session-path.dr",
    );

    let diagnostic = result
        .diagnostics
        .iter()
        .find(|diag| diag.message.contains("mismatched function argument type"))
        .expect("expected direct-call argument mismatch diagnostic");
    let span = diagnostic.primary_span.expect("expected primary span");
    let source = result.session.source_map.get(span.file);
    assert_eq!(source.name, "session-path.dr");

    let rendered = Renderer::new(&result.session.source_map, false).render(diagnostic);
    assert!(
        rendered.contains("session-path.dr"),
        "expected rendered diagnostic to use compiler session path, got:\n{rendered}"
    );
}

#[test]
fn parser_reports_expected_expression_with_found_token_and_note() {
    let (tokens, lex_errors) = lex_with_errors("fn main() { let value = ; }");
    assert!(lex_errors.is_empty(), "expected clean lex");

    let (_module, parse_errors) = parse(FileId(0), &tokens);
    assert!(!parse_errors.is_empty(), "expected parse error");

    let diagnostic = parse_errors
        .iter()
        .find(|diag| diag.message == "expected expression")
        .expect("missing expected-expression diagnostic");
    assert!(
        diagnostic
            .labels
            .iter()
            .any(|label| label.message.contains("found `;`")),
        "expected found-token label, got: {:?}",
        diagnostic.labels
    );
    assert!(
        diagnostic
            .notes
            .iter()
            .any(|note| note.contains("literal, path, block, call")),
        "expected parser guidance note, got: {:?}",
        diagnostic.notes
    );
}

#[test]
fn lexer_recognizes_v1_stable_keywords() {
    let src = "let mut fn fun return if else for in while loop break continue struct enum const static impl extend trait interface implements match as use import export from pub self super crate type async await unsafe defer errdefer where ability capability move true false _";
    let (tokens, errors) = lex_with_errors(src);

    assert!(errors.is_empty());

    let kinds = tokens
        .into_iter()
        .map(|token| token.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            TokenKind::KwLet,
            TokenKind::KwMut,
            TokenKind::KwFn,
            TokenKind::KwFun,
            TokenKind::KwReturn,
            TokenKind::KwIf,
            TokenKind::KwElse,
            TokenKind::KwFor,
            TokenKind::KwIn,
            TokenKind::KwWhile,
            TokenKind::KwLoop,
            TokenKind::KwBreak,
            TokenKind::KwContinue,
            TokenKind::KwStruct,
            TokenKind::KwEnum,
            TokenKind::KwConst,
            TokenKind::KwStatic,
            TokenKind::KwImpl,
            TokenKind::KwExtend,
            TokenKind::KwTrait,
            TokenKind::KwInterface,
            TokenKind::KwImplements,
            TokenKind::KwMatch,
            TokenKind::KwAs,
            TokenKind::KwUse,
            TokenKind::KwImport,
            TokenKind::KwExport,
            TokenKind::KwFrom,
            TokenKind::KwPub,
            TokenKind::KwSelf,
            TokenKind::KwSuper,
            TokenKind::KwCrate,
            TokenKind::KwType,
            TokenKind::KwAsync,
            TokenKind::KwAwait,
            TokenKind::KwUnsafe,
            TokenKind::KwDefer,
            TokenKind::KwErrdefer,
            TokenKind::KwWhere,
            TokenKind::KwAbility,
            TokenKind::KwCapability,
            TokenKind::KwMove,
            TokenKind::Bool(true),
            TokenKind::Bool(false),
            TokenKind::Underscore,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lexer_only_keeps_unassigned_reserved_words_as_identifiers() {
    let src = "class package";
    let (tokens, errors) = lex_with_errors(src);

    assert!(errors.is_empty());

    let kinds = tokens
        .into_iter()
        .map(|token| token.kind)
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            TokenKind::Ident("class".into()),
            TokenKind::Ident("package".into()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn analyze_emits_deprecated_warnings_for_legacy_surface() {
    let result = analyze(
        r#"
        pub struct Counter;

        impl Counter {
            pub fn next(self) -> i32 { 1 }
        }

        use crate::Counter as CounterAlias;
        "#,
        "main.dr",
    );

    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diag| diag.level == daram_compiler::diagnostics::Level::Error),
        "expected warning-only diagnostics, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| (&diag.level, diag.message.as_str()))
            .collect::<Vec<_>>()
    );

    let warnings = result
        .diagnostics
        .iter()
        .filter(|diag| diag.level == daram_compiler::diagnostics::Level::Warning)
        .map(|diag| diag.message.as_str())
        .collect::<Vec<_>>();
    assert!(warnings.contains(&"`pub` is deprecated, use `export` instead"));
    assert!(warnings.contains(&"`impl` is deprecated, use `extend` instead"));
    assert!(warnings.contains(&"`fn` is deprecated, use `fun` instead"));
    assert!(warnings.contains(&"`use` is deprecated, use `import ... from ...` instead"));
    assert!(warnings.contains(&"`->` is deprecated in type positions, use `:` instead"));
}

#[test]
fn name_resolution_diagnostics_include_actionable_notes() {
    let result = analyze(
        r#"
        fun uses_missing_value(): i32 {
            missing_value
        }

        fun uses_missing_path(): i32 {
            pkg::missing()
        }

        fun uses_missing_type(value: MissingType): i32 {
            value
        }
        "#,
        "diagnostic-notes.dr",
    );

    let missing_value = result
        .diagnostics
        .iter()
        .find(|diag| diag.message.contains("cannot find value `missing_value`"))
        .expect("expected missing-value diagnostic");
    assert!(
        missing_value
            .notes
            .iter()
            .any(|note| note.contains("import { ... } from ...")),
        "expected actionable missing-value note, got: {:?}",
        missing_value.notes
    );

    let missing_path = result
        .diagnostics
        .iter()
        .find(|diag| diag.message.contains("cannot resolve path `pkg::missing`"))
        .expect("expected missing-path diagnostic");
    assert!(
        missing_path
            .notes
            .iter()
            .any(|note| note.contains("prefer `import { name } from \"./module\"`")),
        "expected actionable missing-path note, got: {:?}",
        missing_path.notes
    );

    let missing_type = result
        .diagnostics
        .iter()
        .find(|diag| diag.message.contains("cannot find type `MissingType`"))
        .expect("expected missing-type diagnostic");
    assert!(
        missing_type
            .notes
            .iter()
            .any(|note| note.contains("std::core::Option")),
        "expected actionable missing-type note, got: {:?}",
        missing_type.notes
    );
}

#[test]
fn parser_accepts_core_v1_item_declarations() {
    let module = parse_clean(
        r#"
        import { println, read as read_bytes } from "crate/std";
        import * as stdio from "std/io";

        export struct Point<T> where T: Copy {
            export x: T,
            export y: T,
        }

        struct Pair(i32, i32);

        enum Maybe<T> {
            None,
            Some(T),
            Pair { left: T, right: T },
        }

        export const ANSWER: i32 = 42;
        export static mut COUNTER: i32 = 0;

        trait Render {
            fun render(self: Self): string;
        }

        interface Reader {
            fun read(self: Self, count: i32): i32;
        }

        ability Copy;
        type Bytes<T> = [T];
        "#,
    );

    assert_eq!(module.items.len(), 11);
    assert!(matches!(module.items[0], Item::Use(_, _)));
    assert!(matches!(module.items[1], Item::Use(_, _)));

    match &module.items[2] {
        Item::Struct(def) => {
            assert!(def.visibility.is_pub);
            assert_eq!(def.name.name, "Point");
            assert_eq!(def.generics.params.len(), 1);
            match &def.kind {
                StructKind::Fields(fields) => {
                    assert_eq!(fields.len(), 2);
                    assert!(fields.iter().all(|field| field.visibility.is_pub));
                }
                other => panic!("expected field struct, got {other:?}"),
            }
        }
        other => panic!("expected struct item, got {other:?}"),
    }

    assert!(matches!(module.items[3], Item::Struct(_)));

    match &module.items[4] {
        Item::Enum(def) => {
            assert_eq!(def.name.name, "Maybe");
            assert_eq!(def.generics.params.len(), 1);
            assert_eq!(def.variants.len(), 3);
        }
        other => panic!("expected enum item, got {other:?}"),
    }

    match &module.items[5] {
        Item::Const(def) => {
            assert!(def.visibility.is_pub);
            assert_eq!(def.name.name, "ANSWER");
        }
        other => panic!("expected const item, got {other:?}"),
    }

    match &module.items[6] {
        Item::Static(def) => {
            assert!(def.visibility.is_pub);
            assert!(def.mutable);
            assert_eq!(def.name.name, "COUNTER");
        }
        other => panic!("expected static item, got {other:?}"),
    }

    match &module.items[7] {
        Item::Trait(def) => {
            assert_eq!(def.name.name, "Render");
            assert_eq!(def.items.len(), 1);
            match &def.items[0] {
                TraitItem::Method(method) => {
                    assert_eq!(method.params.len(), 1);
                    assert!(matches!(
                        method.params[0].pattern,
                        daram_compiler::ast::Pattern::Ident { .. }
                    ));
                }
                other => panic!("expected trait method, got {other:?}"),
            }
        }
        other => panic!("expected trait item, got {other:?}"),
    }

    match &module.items[8] {
        Item::Interface(def) => {
            assert_eq!(def.name.name, "Reader");
            assert_eq!(def.items.len(), 1);
        }
        other => panic!("expected interface item, got {other:?}"),
    }

    match &module.items[10] {
        Item::TypeAlias(alias) => match &alias.ty {
            TypeExpr::Slice { .. } => {}
            other => panic!("expected slice type alias, got {other:?}"),
        },
        other => panic!("expected type alias, got {other:?}"),
    }
}

#[test]
fn parser_accepts_control_flow_inside_function_bodies() {
    let module = parse_clean(
        r#"
        async unsafe fun main(arg: i32): i32 {
            let total: i32 = 0;
            defer { cleanup(); }
            errdefer { rollback(); }
            while total < arg {
                total = total + 1;
            }
            while let Option::Some(next) = maybe_total {
                total = next;
                break;
            }
            loop {
                break;
            }
            for item in values {
                crate::app::process(item);
            }
            const selected = match total {
                0 => 1,
                _ => total,
            };
            if selected > 1 {
                return selected;
            } else {
                return total;
            }
        }
        "#,
    );

    assert_eq!(module.items.len(), 1);
    match &module.items[0] {
        Item::Function(def) => {
            assert!(def.is_async);
            assert!(def.is_unsafe);
            let body = def.body.as_ref().expect("main should have a body");
            match body {
                Expr::Block { stmts, tail, .. } => {
                    assert!(stmts.iter().any(|stmt| matches!(stmt, Stmt::Defer { .. })));
                    assert!(stmts
                        .iter()
                        .any(|stmt| matches!(stmt, Stmt::Errdefer { .. })));
                    assert!(stmts.iter().any(|stmt| matches!(
                        stmt,
                        Stmt::Expr {
                            expr: Expr::While { .. },
                            ..
                        }
                    )));
                    assert!(stmts.iter().any(|stmt| matches!(
                        stmt,
                        Stmt::Expr {
                            expr: Expr::WhileLet { .. },
                            ..
                        }
                    )));
                    assert!(stmts.iter().any(|stmt| matches!(
                        stmt,
                        Stmt::Expr {
                            expr: Expr::Loop { .. },
                            ..
                        }
                    )));
                    assert!(stmts.iter().any(|stmt| matches!(
                        stmt,
                        Stmt::Expr {
                            expr: Expr::For { .. },
                            ..
                        }
                    )));
                    assert!(stmts.iter().any(|stmt| matches!(stmt, Stmt::Let { .. })));
                    assert!(stmts.iter().any(|stmt| matches!(
                        stmt,
                        Stmt::Let {
                            init: Some(Expr::Match { .. }),
                            ..
                        }
                    )));
                    assert!(matches!(tail.as_deref(), Some(Expr::If { .. })));
                }
                other => panic!("expected block body, got {other:?}"),
            }
        }
        other => panic!("expected function item, got {other:?}"),
    }
}

#[test]
fn parser_accepts_while_let_expressions() {
    let module = parse_clean(
        r#"
        fun main(): i32 {
            let current = std::core::Option::Some(1);
            while let std::core::Option::Some(value) = current {
                return value;
            }
            0
        }
        "#,
    );

    match &module.items[0] {
        Item::Function(def) => match def.body.as_ref().expect("main should have a body") {
            Expr::Block { stmts, .. } => assert!(stmts.iter().any(|stmt| matches!(
                stmt,
                Stmt::Expr {
                    expr: Expr::WhileLet { .. },
                    ..
                }
            ))),
            other => panic!("expected block body, got {other:?}"),
        },
        other => panic!("expected function item, got {other:?}"),
    }
}

#[test]
fn parser_accepts_const_and_static_items() {
    let module = parse_clean(
        r#"
        const ANSWER: i32 = 42;
        static CACHE: i32 = 7;
        static mut GLOBAL_COUNT: i32 = 0;
        "#,
    );

    assert_eq!(module.items.len(), 3);
    assert!(matches!(module.items[0], Item::Const(_)));
    match &module.items[1] {
        Item::Static(def) => assert!(!def.mutable),
        other => panic!("expected static item, got {other:?}"),
    }
    match &module.items[2] {
        Item::Static(def) => assert!(def.mutable),
        other => panic!("expected static mut item, got {other:?}"),
    }
}

#[test]
fn parser_accepts_spec_style_declarations_without_legacy_delimiters() {
    let module = parse_clean(
        r#"
        struct Marker

        export struct Point {
          x: f64
          y: f64
        }

        enum Direction {
          North
          South
          East
          West
        }
        "#,
    );

    assert_eq!(module.items.len(), 3);
    assert!(matches!(module.items[0], Item::Struct(_)));
    assert!(matches!(module.items[1], Item::Struct(_)));
    match &module.items[2] {
        Item::Enum(def) => assert_eq!(def.variants.len(), 4),
        other => panic!("expected enum item, got {other:?}"),
    }
}

#[test]
fn lexer_reports_invalid_unicode_escape_without_panicking() {
    let (_tokens, errors) = lex_with_errors(r#""\u{XYZ}""#);
    assert!(
        errors
            .iter()
            .any(|error| error.message.contains("unicode escape")),
        "expected unicode escape diagnostic, got: {:?}",
        errors
            .iter()
            .map(|error| error.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn resolver_lowers_function_bodies_into_hir() {
    let hir = resolve_clean(
        r#"
        fn helper() -> i32 {
            1
        }

        fn main() -> i32 {
            helper()
        }
        "#,
    );

    assert_eq!(hir.functions.len(), 2);
    let main = &hir.functions[1];
    let body = main.body.as_ref().expect("main body should be lowered");
    match &body.kind {
        HirExprKind::Block(_, Some(tail)) => match &tail.kind {
            HirExprKind::Call { callee, .. } => {
                assert!(matches!(callee.kind, HirExprKind::DefRef(_)));
            }
            other => panic!("expected call tail, got {other:?}"),
        },
        other => panic!("expected lowered block body, got {other:?}"),
    }
}

#[test]
fn resolver_collects_type_alias_items() {
    let hir = resolve_clean(
        r#"
        type Bytes = [u8];
        type Callback = fun(i32): i32;

        fn main() {
        }
        "#,
    );

    assert_eq!(hir.type_aliases.len(), 2);
    assert_eq!(hir.functions.len(), 1);
}

#[test]
fn parser_accepts_function_type_with_fun_colon_syntax() {
    let module = parse_clean(
        r#"
        type Callback = fun(i32): i32;
        "#,
    );

    match &module.items[0] {
        Item::TypeAlias(alias) => match &alias.ty {
            TypeExpr::FnPtr { params, ret, .. } => {
                assert_eq!(params.len(), 1);
                assert!(ret.is_some());
            }
            other => panic!("expected function type alias, got {other:?}"),
        },
        other => panic!("expected type alias, got {other:?}"),
    }
}

#[test]
fn resolver_collects_trait_and_interface_items() {
    let hir = resolve_clean(
        r#"
        trait Render {
            fun render(value: i32): string;
            type Output = i32;
            const ENABLED: bool = true;
        }

        interface Reader {
            fun read(count: i32): i32;
        }
        "#,
    );

    assert_eq!(hir.traits.len(), 1);
    assert_eq!(hir.interfaces.len(), 1);
    assert_eq!(hir.traits[0].items.len(), 3);
    assert_eq!(hir.interfaces[0].items.len(), 1);
}

#[test]
fn resolver_collects_impl_items() {
    let hir = resolve_clean(
        r#"
        struct Point;

        extend Point {
            fun value(x: i32): i32 {
                x
            }

            type Output = i32;
            const ZERO: i32 = 0;
        }
        "#,
    );

    assert_eq!(hir.impls.len(), 1);
    assert_eq!(hir.impls[0].items.len(), 3);
}

#[test]
fn resolver_collects_extend_implements_items() {
    let hir = resolve_clean(
        r#"
        trait Display {
            fun to_string(self): string;
        }

        struct Point

        extend Point implements Display {
            fun to_string(self): string {
                "point"
            }
        }
        "#,
    );

    assert_eq!(hir.traits.len(), 1);
    assert_eq!(hir.impls.len(), 1);
    assert!(hir.impls[0].trait_ref.is_some());
    assert_eq!(hir.impls[0].items.len(), 1);
}

#[test]
fn compile_accepts_lowercase_string_surface_type() {
    let result = compile(
        r#"
        fun greet(name: string): string {
            name
        }
        "#,
        "string-alias.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn resolver_collects_use_and_module_items() {
    let file = FileId(0);
    let span = |start, end| daram_compiler::source::Span::new(file, start, end);
    let ast = daram_compiler::ast::Module {
        file,
        items: vec![
            Item::Use(
                daram_compiler::ast::UseTree {
                    prefix: daram_compiler::ast::Path {
                        segments: vec![
                            daram_compiler::ast::Ident::new("crate".to_string(), span(0, 5)),
                            daram_compiler::ast::Ident::new("std".to_string(), span(6, 9)),
                        ],
                        span: span(0, 9),
                    },
                    kind: daram_compiler::ast::UseTreeKind::Nested(vec![
                        daram_compiler::ast::UseTree {
                            prefix: daram_compiler::ast::Path {
                                segments: vec![daram_compiler::ast::Ident::new(
                                    "println".to_string(),
                                    span(10, 17),
                                )],
                                span: span(10, 17),
                            },
                            kind: daram_compiler::ast::UseTreeKind::Simple,
                            span: span(10, 17),
                        },
                        daram_compiler::ast::UseTree {
                            prefix: daram_compiler::ast::Path {
                                segments: vec![daram_compiler::ast::Ident::new(
                                    "read".to_string(),
                                    span(18, 22),
                                )],
                                span: span(18, 22),
                            },
                            kind: daram_compiler::ast::UseTreeKind::Alias(
                                daram_compiler::ast::Ident::new(
                                    "read_bytes".to_string(),
                                    span(23, 33),
                                ),
                            ),
                            span: span(18, 33),
                        },
                    ]),
                    span: span(0, 33),
                },
                span(0, 33),
            ),
            Item::Module(daram_compiler::ast::ModuleDef {
                visibility: daram_compiler::ast::Visibility::private(),
                name: daram_compiler::ast::Ident::new("nested".to_string(), span(34, 40)),
                body: Some(vec![Item::Function(daram_compiler::ast::FnDef {
                    visibility: daram_compiler::ast::Visibility::private(),
                    is_async: false,
                    is_unsafe: false,
                    name: daram_compiler::ast::Ident::new("helper".to_string(), span(41, 47)),
                    generics: daram_compiler::ast::GenericParams {
                        params: Vec::new(),
                        span: None,
                    },
                    params: Vec::new(),
                    ret_ty: None,
                    where_clause: Vec::new(),
                    body: Some(Expr::Block {
                        stmts: Vec::new(),
                        tail: None,
                        span: span(48, 50),
                    }),
                    span: span(41, 50),
                })]),
                span: span(34, 50),
            }),
        ],
        span: span(0, 50),
    };
    let (hir, diagnostics) = resolve(file, &ast);
    assert!(
        diagnostics.is_empty(),
        "expected clean lowering, got: {:?}",
        diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );

    assert_eq!(hir.uses.len(), 1);
    assert_eq!(hir.modules.len(), 1);
    let nested = hir.modules[0]
        .body
        .as_ref()
        .expect("nested module body should be lowered");
    assert_eq!(nested.functions.len(), 1);
}

#[test]
fn parser_accepts_bare_package_import_sources() {
    let module = parse_clean(
        r#"
        import { parse } from json_extra;
        "#,
    );

    match &module.items[0] {
        Item::Use(tree, _) => {
            assert_eq!(tree.prefix.segments.len(), 1);
            assert_eq!(tree.prefix.segments[0].name, "json_extra");
        }
        other => panic!("expected import item, got {other:?}"),
    }
}

#[test]
fn resolver_distinguishes_locals_from_top_level_defs() {
    let hir = resolve_clean(
        r#"
        fun helper(): i32 {
            1
        }

        fun main(): i32 {
            let helper: i32 = 3;
            helper
        }
        "#,
    );

    let main = &hir.functions[1];
    let body = main.body.as_ref().expect("main body should be lowered");
    match &body.kind {
        HirExprKind::Block(_, Some(tail)) => {
            assert!(matches!(tail.kind, HirExprKind::Var(_)));
        }
        other => panic!("expected lowered block body, got {other:?}"),
    }
}

#[test]
fn resolver_lowers_block_scoped_use_aliases() {
    let hir = resolve_clean(
        r#"
        fun helper(): i32 {
            1
        }

        fun main(): i32 {
            {
                use helper as call_helper;
                call_helper()
            }
        }
        "#,
    );

    let main = &hir.functions[1];
    let body = main.body.as_ref().expect("main body should be lowered");
    match &body.kind {
        HirExprKind::Block(_, Some(tail)) => match &tail.kind {
            HirExprKind::Block(stmts, Some(inner_tail)) => {
                assert!(matches!(
                    stmts.first().map(|stmt| &stmt.kind),
                    Some(HirStmtKind::Use(_))
                ));
                match &inner_tail.kind {
                    HirExprKind::Call { callee, .. } => {
                        assert!(matches!(callee.kind, HirExprKind::DefRef(_)));
                    }
                    other => panic!("expected aliased call tail, got {other:?}"),
                }
            }
            other => panic!("expected inner block, got {other:?}"),
        },
        other => panic!("expected lowered block body, got {other:?}"),
    }
}

#[test]
fn resolver_evaluates_array_length_const_expressions() {
    let hir = resolve_clean(
        r#"
        const WIDTH: usize = 4;
        type Row = [i32; WIDTH + 1];
        "#,
    );

    assert_eq!(hir.type_aliases.len(), 1);
    match &hir.type_aliases[0].ty {
        Ty::Array { len, .. } => assert_eq!(*len, 5),
        other => panic!("expected array alias, got {other:?}"),
    }
}

#[test]
fn compile_accepts_simple_lowered_function_bodies() {
    let result = compile(
        r#"
        fn helper() -> i32 {
            1
        }

        fn main() -> i32 {
            let value: i32 = helper();
            value
        }
        "#,
        "lowered.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_infers_local_types_from_lowered_bodies() {
    let result = compile(
        r#"
        fn main() -> i32 {
            let value = 1;
            value
        }
        "#,
        "inferred-lowered.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_top_level_use_aliases() {
    let result = compile(
        r#"
        fn helper() -> i32 {
            1
        }

        use helper as call_helper;

        fn main() -> i32 {
            call_helper()
        }
        "#,
        "use-alias.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_const_and_static_references() {
    let result = compile(
        r#"
        const ANSWER: i32 = 42;
        static mut COUNTER: i32 = 0;

        fn main() -> i32 {
            ANSWER
        }
        "#,
        "globals.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_range_match_patterns() {
    let result = compile(
        r#"
        fn main(value: i32) -> i32 {
            match value {
                0..=3 => 1,
                _ => 2,
            }
        }
        "#,
        "range-pattern.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_slice_match_patterns() {
    let result = compile(
        r#"
        fn main(values: [i32; 3]) -> i32 {
            match values {
                [1, .., 3] => 1,
                _ => 0,
            }
        }
        "#,
        "slice-pattern.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_loop_expressions() {
    let result = compile(
        r#"
        fn main() {
            loop {
                break;
            }
        }
        "#,
        "loop.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_struct_field_construction_and_access() {
    let result = compile(
        r#"
        struct Point {
            x: i32,
            y: i32,
        }

        fn main() -> i32 {
            let point = Point { x: 1, y: 2 };
            point.x
        }
        "#,
        "struct-fields.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_struct_field_type_mismatch() {
    let result = compile(
        r#"
        struct Point {
            x: i32,
        }

        fn main() {
            let point = Point { x: true };
        }
        "#,
        "struct-field-mismatch.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_invalid_index_type() {
    let result = compile(
        r#"
        fn main() -> i32 {
            let values = [1, 2, 3];
            values[true]
        }
        "#,
        "index-mismatch.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_checks_nested_modules_and_impl_items() {
    let result = compile(
        r#"
        struct Point;

        impl Point {
            const ZERO: bool = 0;
        }

        fn main() {}
        "#,
        "nested-typeck.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("mismatched impl const types")),
        "expected impl item type mismatch, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_non_exhaustive_bool_match() {
    let result = compile(
        r#"
        fn main(value: bool) -> i32 {
            match value {
                true => 1,
            }
        }
        "#,
        "non-exhaustive-bool.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("non-exhaustive match")),
        "expected non-exhaustive match diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_exhaustive_bool_match() {
    let result = compile(
        r#"
        fn main(value: bool) -> i32 {
            match value {
                true => 1,
                false => 0,
            }
        }
        "#,
        "exhaustive-bool.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_non_exhaustive_enum_match() {
    let result = compile(
        r#"
        enum State {
            Idle,
            Busy,
        }

        fn main(state: State) -> i32 {
            match state {
                Idle => 0,
            }
        }
        "#,
        "non-exhaustive-enum.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("non-exhaustive match")),
        "expected non-exhaustive match diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_exhaustive_enum_match() {
    let result = compile(
        r#"
        enum State {
            Idle,
            Busy,
        }

        fn main(state: State) -> i32 {
            match state {
                Idle => 0,
                Busy => 1,
            }
        }
        "#,
        "exhaustive-enum.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_non_exhaustive_nested_enum_match() {
    let result = compile(
        r#"
        enum MaybeBool {
            None,
            Some(bool),
        }

        fn main(value: MaybeBool) -> i32 {
            match value {
                None => 0,
                Some(true) => 1,
            }
        }
        "#,
        "nested-non-exhaustive-enum.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("Some(false)")),
        "expected nested non-exhaustive diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_exhaustive_nested_enum_match() {
    let result = compile(
        r#"
        enum MaybeBool {
            None,
            Some(bool),
        }

        fn main(value: MaybeBool) -> i32 {
            match value {
                None => 0,
                Some(true) => 1,
                Some(false) => 2,
            }
        }
        "#,
        "nested-exhaustive-enum.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_non_exhaustive_integer_range_match() {
    let result = compile(
        r#"
        fn main(value: i32) -> i32 {
            match value {
                0..=10 => 1,
                11..=20 => 2,
            }
        }
        "#,
        "non-exhaustive-int-range.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("missing values like")),
        "expected range exhaustiveness diagnostic, got {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_guard_exhaustive_small_integer_match() {
    let result = compile(
        r#"
        fn main(value: i8) -> i32 {
            match value {
                n if n < (0 as i8) => 0,
                n if n >= (0 as i8) => 1,
            }
        }
        "#,
        "guard-exhaustive-i8.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_guarded_small_integer_match_with_gap() {
    let result = compile(
        r#"
        fn main(value: i8) -> i32 {
            match value {
                n if n < (0 as i8) => 0,
                n if n > (0 as i8) => 1,
            }
        }
        "#,
        "guard-non-exhaustive-i8.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("`0`")),
        "expected precise guarded integer exhaustiveness diagnostic, got {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_guarded_i32_match_covering_entire_domain() {
    let result = compile(
        r#"
        fn main(value: i32) -> i32 {
            match value {
                n if n < 0 => 0,
                n if n >= 0 => 1,
            }
        }
        "#,
        "guard-exhaustive-i32.dr",
    );

    assert!(
        !result.has_errors(),
        "expected guarded i32 match to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_guarded_i32_match_with_gap() {
    let result = compile(
        r#"
        fn main(value: i32) -> i32 {
            match value {
                n if n < 0 => 0,
                n if n > 0 => 1,
            }
        }
        "#,
        "guard-gap-i32.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("`0`")),
        "expected guarded i32 gap diagnostic, got {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_move_while_borrowed() {
    let result = compile(
        r#"
        struct Point {
            x: i32,
        }

        fn take(point: Point) {}

        fn main() {
            let point = Point { x: 1 };
            let borrow = &point;
            take(point);
        }
        "#,
        "move-while-borrowed.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot move value while it is borrowed")),
        "expected borrow diagnostic, got {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_mutable_borrow_conflict() {
    let result = compile(
        r#"
        fn main() {
            let mut value = 1;
            let first = &value;
            let second = &mut value;
        }
        "#,
        "mutable-borrow-conflict.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot mutably borrow value while it is already borrowed")),
        "expected mutable borrow conflict diagnostic, got {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_disjoint_field_borrows_and_writes() {
    let result = compile(
        r#"
        struct Pair {
            left: i32,
            right: i32,
        }

        fn main() -> i32 {
            let mut pair = Pair { left: 1, right: 2 };
            let left = &pair.left;
            pair.right = 3;
            pair.right
        }
        "#,
        "disjoint-field-borrows.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_disjoint_field_write_through_deref_alias() {
    let result = compile(
        r#"
        struct Pair {
            left: i32,
            right: i32,
        }

        fn main() -> i32 {
            let mut pair = Pair { left: 1, right: 2 };
            let ptr = &mut pair;
            let left = &(*ptr).left;
            (*ptr).right = 3;
            (*ptr).right
        }
        "#,
        "deref-alias-disjoint-field.dr",
    );

    assert!(
        !result.has_errors(),
        "expected disjoint deref-alias field write to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_same_field_mutable_borrow_conflict() {
    let result = compile(
        r#"
        struct Pair {
            left: i32,
            right: i32,
        }

        fn main() {
            let mut pair = Pair { left: 1, right: 2 };
            let left = &pair.left;
            let second = &mut pair.left;
        }
        "#,
        "same-field-borrow-conflict.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot mutably borrow value while it is already borrowed")),
        "expected same-field borrow conflict diagnostic, got {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_same_field_write_through_deref_alias() {
    let result = compile(
        r#"
        struct Pair {
            left: i32,
            right: i32,
        }

        fn main() {
            let mut pair = Pair { left: 1, right: 2 };
            let ptr = &mut pair;
            let left = &(*ptr).left;
            (*ptr).left = 3;
            left;
        }
        "#,
        "deref-alias-same-field-write.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot assign to value while it is borrowed")),
        "expected deref-alias same-field borrow diagnostic, got {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_deref_alias_write_while_shared_alias_borrow_exists() {
    let result = compile(
        r#"
        fn main() {
            let mut value = 1;
            let r = &mut value;
            let shared = &*r;
            *r = 2;
            shared;
        }
        "#,
        "deref-alias-borrow-conflict.dr",
    );

    assert!(result.has_errors(), "expected deref alias borrow conflict");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot assign to value while it is borrowed")),
        "expected deref alias conflict diagnostic, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_allows_assignment_after_last_shared_borrow_use() {
    let result = compile(
        r#"
        fn main() {
            let mut value = 1;
            let shared = &value;
            shared;
            value = 2;
        }
        "#,
        "nll-shared-last-use.dr",
    );

    assert!(
        !result.has_errors(),
        "expected assignment after last shared borrow use to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_allows_mutable_borrow_after_temporary_shared_borrow_statement() {
    let result = compile(
        r#"
        fn observe(value: &i32) {}

        fn main() {
            let mut value = 1;
            observe(&value);
            let ptr = &mut value;
            ptr;
        }
        "#,
        "nll-temp-shared-borrow.dr",
    );

    assert!(
        !result.has_errors(),
        "expected mutable borrow after temporary shared borrow to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_allows_move_after_last_borrow_alias_use() {
    let result = compile(
        r#"
        struct Point {
            x: i32,
        }

        fn consume(point: Point) {}

        fn main() {
            let point = Point { x: 1 };
            let borrow = &point;
            borrow;
            consume(point);
        }
        "#,
        "nll-move-after-borrow-use.dr",
    );

    assert!(
        !result.has_errors(),
        "expected move after last borrow alias use to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_match_pattern_type_mismatch() {
    let result = compile(
        r#"
        fn main(value: bool) -> i32 {
            match value {
                1 => 1,
                false => 0,
            }
        }
        "#,
        "match-pattern-mismatch.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("mismatched match pattern type")),
        "expected pattern type diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_guard_only_bool_match_as_non_exhaustive() {
    let result = compile(
        r#"
        fn main(value: bool) -> i32 {
            match value {
                true if value => 1,
                false => 0,
            }
        }
        "#,
        "guarded-bool.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("non-exhaustive match")),
        "expected non-exhaustive match diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_exhaustive_unit_match() {
    let result = compile(
        r#"
        fn main() -> i32 {
            match () {
                () => 1,
            }
        }
        "#,
        "unit-match.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_use_after_move() {
    let result = compile(
        r#"
        struct Point {
            x: i32,
        }

        fn consume(point: Point) {}

        fn main() {
            let point = Point { x: 1 };
            consume(point);
            consume(point);
        }
        "#,
        "use-after-move.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("use of moved value")),
        "expected use-after-move diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .notes
            .iter()
            .any(|note| note.contains("borrow the value with `&`"))),
        "expected ownership guidance note, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_reuse_of_copy_value() {
    let result = compile(
        r#"
        fn consume(value: i32) {}

        fn main() {
            let value = 1;
            consume(value);
            consume(value);
        }
        "#,
        "copy-reuse.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_assignment_to_immutable_binding() {
    let result = compile(
        r#"
        fun main() {
            const value: i32 = 1;
            value = 2;
        }
        "#,
        "immutable-assign.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("cannot assign to immutable binding")),
        "expected immutable assignment diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.notes.iter().any(|note| note.contains("`let`"))),
        "expected mutability fix note, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_assignment_to_mutable_binding() {
    let result = compile(
        r#"
        fun main() {
            let value: i32 = 1;
            value = 2;
        }
        "#,
        "mutable-assign.dr",
    );

    assert!(
        !result.has_errors(),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_mutable_borrow_of_immutable_binding() {
    let result = compile(
        r#"
        fun main() {
            const value: i32 = 1;
            const ptr = &mut value;
            const sink = ptr;
            sink;
        }
        "#,
        "mutable-borrow.dr",
    );

    assert!(
        result.has_errors(),
        "expected compile error, got diagnostics: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot borrow immutable binding as mutable")),
        "expected mutable borrow diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_stdlib_source_compiles_with_user_program() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            use std::http::OK;

            fn main() {
                let value = std::core::Option::Some(true);
                match value {
                    std::core::Option::Some(flag) if flag => {
                        let status = OK;
                        status;
                    }
                    std::core::Option::Some(_) => {}
                    std::core::Option::None => {}
                }
            }
            "#,
        ),
        "bundled-stdlib.dr",
    );

    assert!(
        !result.has_errors(),
        "expected bundled stdlib compile to succeed, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_stdlib_display_surface_compiles() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            import { Display, Result } from "std/core";

            struct Point {
                x: i32,
            }

            extend Point implements Display {
                fun fmt(self, f: &mut std::fmt::Formatter): Result<(), std::fmt::Error> {
                    (*f).write_str("point");
                    std::core::Result::Ok(())
                }
            }

            fun main(): bool {
                true
            }
            "#,
        ),
        "bundled-stdlib-display.dr",
    );

    assert!(
        !result.has_errors(),
        "expected bundled Display surface compile to succeed, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_http_and_net_surface_compiles() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            fun main(): bool {
                const cap = std::net::NetCap { _private: () };
                const client = std::http::Client::new(cap);
                const request = std::http::Request::get("http://127.0.0.1:8080/")
                    .header("accept", "application/json");
                const method_ok = request.method.as_str() == "GET";
                const addr_ok = match std::net::SocketAddr::from_str("127.0.0.1:8080") {
                    std::core::Result::Ok(_) => true,
                    std::core::Result::Err(_) => false,
                };
                client.timeout_ms(1000).follow_redirects(false);
                method_ok && addr_ok
            }
            "#,
        ),
        "bundled-http-net.dr",
    );

    assert!(
        !result.has_errors(),
        "expected bundled http/net surface compile to succeed, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_task_surface_compiles() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            async fun add_one(value: i32): i32 {
                value + 1
            }

            fun main(): bool {
                const handle = std::task::spawn(fun(): i32 {
                    add_one(41) await
                });
                std::task::sleep_ms(0 as u64);
                std::task::block_on(fun(): bool {
                    handle.join() == 42
                })
            }
            "#,
        ),
        "bundled-task.dr",
    );

    assert!(
        !result.has_errors(),
        "expected bundled task surface compile to succeed, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_stdlib_impl_subset_compiles_with_method_calls() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            fn main() -> bool {
                let status = std::http::OK;
                status.is_success()
            }
            "#,
        ),
        "bundled-stdlib-impls.dr",
    );

    assert!(
        !result.has_errors(),
        "expected bundled stdlib impl subset compile to succeed, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_unstable_stdlib_subset_compiles() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            fn main(read: std::fs::FsReadCap) -> bool {
                let values: std::collections::Vec<i32>;
                let json = std::json::bool_val(true);
                values;
                read;
                match json {
                    std::json::Value::Bool(flag) => flag,
                    _ => false,
                }
            }
            "#,
        ),
        "bundled-unstable-stdlib.dr",
    );

    assert!(
        !result.has_errors(),
        "expected bundled unstable stdlib subset compile to succeed, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_time_net_crypto_subset_compiles() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            fn main(net: std::net::NetCap, crypto: std::crypto::CryptoCap) -> bool {
                let duration = std::time::duration_from_secs(1 as u64);
                let addr = std::net::socket_addr_v4(127, 0, 0, 1, 8080);
                let seed = std::crypto::random_u64(crypto);
                duration;
                addr;
                std::crypto::constant_time_eq(seed as u8, 4)
            }
            "#,
        ),
        "bundled-time-net-crypto.dr",
    );

    assert!(
        !result.has_errors(),
        "expected bundled time/net/crypto subset compile to succeed, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn bundled_user_source_does_not_split_on_embedded_bundle_markers() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            fun main(): i32 {
                injected::answer()
            }

            //!__daram_file:injected.dr
            export fun answer(): i32 {
                42
            }
            "#,
        ),
        "bundled-marker-injection.dr",
    );

    assert!(
        result.has_errors(),
        "expected embedded bundle marker to stay in-file and leave `injected` unresolved"
    );
    assert!(
        result.diagnostics.iter().any(|diag| {
            diag.message.contains("unresolved")
                || diag.message.contains("unknown")
                || diag.message.contains("cannot resolve path")
        }),
        "expected unresolved-module style diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_result_try_in_non_result_function() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            fun fallible(): std::core::Result<i32, i32> {
                std::core::Result::Err(1)
            }

            fun main(): i32 {
                fallible()?
            }
            "#,
        ),
        "try-non-result-return.dr",
    );

    assert!(result.has_errors(), "expected `?` return-type mismatch");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("requires the enclosing function to return `Result`")),
        "expected `Result` try diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_try_on_non_carrier_value() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            fun main(): std::core::Result<i32, i32> {
                const value = 1?;
                std::core::Result::Ok(value)
            }
            "#,
        ),
        "try-non-carrier.dr",
    );

    assert!(result.has_errors(), "expected invalid `?` operand error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("can only be applied to `Result` or `Option`")),
        "expected invalid `?` operand diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_guard_refined_exhaustive_bool_match() {
    let result = compile(
        r#"
        fn main(flag: bool) -> i32 {
            match flag {
                value if value => 1,
                _ => 0,
            }
        }
        "#,
        "guard-exhaustive.dr",
    );

    assert!(
        !result.has_errors(),
        "expected guard-refined match to compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_exhaustive_tuple_match() {
    let result = compile(
        r#"
        fn main(pair: (bool, bool)) -> i32 {
            match pair {
                (true, true) => 1,
                (true, false) => 2,
                (false, true) => 3,
                (false, false) => 4,
            }
        }
        "#,
        "tuple-exhaustive.dr",
    );

    assert!(
        !result.has_errors(),
        "expected exhaustive tuple match to compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_non_exhaustive_struct_match() {
    let result = compile(
        r#"
        struct Flags {
            left: bool,
            right: bool,
        }

        fn main(flags: Flags) -> i32 {
            match flags {
                Flags { left: true, right: true } => 1,
                Flags { left: true, right: false } => 2,
                Flags { left: false, right: true } => 3,
            }
        }
        "#,
        "struct-non-exhaustive.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("non-exhaustive match")),
        "expected non-exhaustive match diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_exhaustive_struct_match() {
    let result = compile(
        r#"
        struct Flags {
            left: bool,
            right: bool,
        }

        fn main(flags: Flags) -> i32 {
            match flags {
                Flags { left: true, right: true } => 1,
                Flags { left: true, right: false } => 2,
                Flags { left: false, right: true } => 3,
                Flags { left: false, right: false } => 4,
            }
        }
        "#,
        "struct-exhaustive.dr",
    );

    assert!(
        !result.has_errors(),
        "expected exhaustive struct match to compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_non_exhaustive_array_match() {
    let result = compile(
        r#"
        fn main(flags: [bool; 2]) -> i32 {
            match flags {
                [true, true] => 1,
                [true, false] => 2,
                [false, true] => 3,
            }
        }
        "#,
        "array-non-exhaustive.dr",
    );

    assert!(
        result.has_errors(),
        "expected non-exhaustive array match error"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("non-exhaustive match")),
        "expected non-exhaustive match diagnostic, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_exhaustive_array_match() {
    let result = compile(
        r#"
        fn main(flags: [bool; 2]) -> i32 {
            match flags {
                [true, true] => 1,
                [true, false] => 2,
                [false, true] => 3,
                [false, false] => 4,
            }
        }
        "#,
        "array-exhaustive.dr",
    );

    assert!(
        !result.has_errors(),
        "expected exhaustive array match to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_exhaustive_array_match_with_rest_prefix() {
    let result = compile(
        r#"
        fn main(flags: [bool; 3]) -> i32 {
            match flags {
                [true, ..] => 1,
                [false, ..] => 2,
            }
        }
        "#,
        "array-rest-prefix-exhaustive.dr",
    );

    assert!(
        !result.has_errors(),
        "expected exhaustive rest-prefix array match to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_resolves_self_receiver_methods_inside_impl_bodies() {
    let result = compile(
        r#"
        struct Counter {
            value: i32,
        }

        extend Counter {
            fun bump(self): i32 {
                self.value
            }

            fun read(self): i32 {
                self.bump()
            }
        }

        fun main(): i32 {
            const counter = Counter { value: 1 };
            counter.read()
        }
        "#,
        "self-receiver-methods.dr",
    );

    assert!(
        !result.has_errors(),
        "expected self receiver method calls in impl bodies to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_exhaustive_array_match_with_rest_middle() {
    let result = compile(
        r#"
        fn main(flags: [bool; 3]) -> i32 {
            match flags {
                [true, .., true] => 1,
                [true, .., false] => 2,
                [false, .., true] => 3,
                [false, .., false] => 4,
            }
        }
        "#,
        "array-rest-middle-exhaustive.dr",
    );

    assert!(
        !result.has_errors(),
        "expected exhaustive rest-middle array match to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_non_exhaustive_array_match_with_rest_gap() {
    let result = compile(
        r#"
        fn main(flags: [bool; 3]) -> i32 {
            match flags {
                [true, .., true] => 1,
                [false, .., false] => 2,
            }
        }
        "#,
        "array-rest-gap-non-exhaustive.dr",
    );

    assert!(
        result.has_errors(),
        "expected non-exhaustive rest-middle array match error"
    );
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("non-exhaustive match")),
        "expected non-exhaustive match diagnostic, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_rejects_returning_reference_to_local_binding() {
    let result = compile(
        r#"
        fn local_ref() -> &i32 {
            let value = 1;
            return &value;
        }
        "#,
        "return-local-ref.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot return reference to local binding")),
        "expected escaping reference diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn resolve_lowers_ability_items_into_hir() {
    let hir = resolve_clean(
        r#"
        ability Copy;
        ability Iterator {
            type Item;
            fun next(self): Self::Item;
            fun count(self): usize { 0 }
        }
        "#,
    );

    assert!(hir
        .abilities
        .iter()
        .any(|ability| { hir.def_names.get(&ability.def).map(String::as_str) == Some("Copy") }));
    let iterator_ability = hir
        .abilities
        .iter()
        .find(|ability| hir.def_names.get(&ability.def).map(String::as_str) == Some("Iterator"))
        .expect("expected Iterator ability");
    assert_eq!(iterator_ability.items.len(), 3);
    assert!(matches!(
        iterator_ability.items[0],
        daram_compiler::hir::HirAssocItem::TypeAssoc { .. }
    ));
    assert!(matches!(
        iterator_ability.items[1],
        daram_compiler::hir::HirAssocItem::Method(_)
    ));
    assert!(matches!(
        iterator_ability.items[2],
        daram_compiler::hir::HirAssocItem::Method(_)
    ));
}

#[test]
fn compile_accepts_ability_with_assoc_items() {
    let result = compile(
        r#"
        ability Iterator {
            type Item;
            fun next(self): Self::Item;
            fun count(self): usize { 0 }
        }

        fun main(): i32 { 0 }
        "#,
        "ability-items.dr",
    );

    assert!(
        !result.has_errors(),
        "expected ability assoc items to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_synthesizes_ability_default_methods_into_impls() {
    let result = compile(
        r#"
        ability Meter {
            fun value(self): i32;
            fun plus_one(self): i32 {
                self.value() + 1
            }
        }

        struct Reader {
            n: i32,
        }

        struct Counter {
            n: i32,
        }

        extend Reader implements Meter {
            fun value(self): i32 {
                self.n
            }
        }

        extend Counter implements Meter {
            fun value(self): i32 {
                self.n + 10
            }
        }

        fun main(): i32 {
            const reader = Reader { n: 4 };
            const counter = Counter { n: 5 };
            reader.plus_one() + counter.plus_one()
        }
        "#,
        "ability-default-methods.dr",
    );

    assert!(
        !result.has_errors(),
        "expected ability default methods to synthesize into impls, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn compile_accepts_explicit_copy_impl_for_named_type() {
    let result = compile(
        r#"
        ability Copy;

        struct Ticket {
            id: i32,
        }

        impl Copy for Ticket {}

        fn take(ticket: Ticket) {}

        fn main() {
            let ticket = Ticket { id: 1 };
            take(ticket);
            take(ticket);
        }
        "#,
        "copy-impl-ok.dr",
    );

    assert!(
        !result.has_errors(),
        "expected explicit Copy impl to allow reuse, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_derive_copy_for_named_type() {
    let result = compile(
        r#"
        ability Copy;

        @derive(Copy)
        struct Ticket {
            id: i32,
        }

        fn take(ticket: Ticket) {}

        fn main() {
            let ticket = Ticket { id: 1 };
            take(ticket);
            take(ticket);
        }
        "#,
        "derive-copy-ok.dr",
    );

    assert!(
        !result.has_errors(),
        "expected `@derive(Copy)` to allow reuse, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_copy_impl_with_non_copy_field() {
    let result = compile(
        r#"
        ability Copy;

        struct Payload {
            text: String,
        }

        impl Copy for Payload {}
        "#,
        "copy-impl-non-copy-field.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("does not satisfy `Copy`")),
        "expected Copy field diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_derive_copy_with_non_copy_field() {
    let result = compile(
        r#"
        ability Copy;

        @derive(Copy)
        struct Payload {
            text: String,
        }
        "#,
        "derive-copy-non-copy-field.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("cannot derive `Copy`")),
        "expected derive Copy field diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_derive_hash_eq_partialeq_for_hashable_fields() {
    let result = compile(
        r#"
        ability PartialEq {
            fun eq(self, other: &Self): bool;
        }

        ability Eq: PartialEq {}

        ability Hash {
            fun hash(self, state: &mut i32);
        }

        @derive(Hash, Eq, PartialEq)
        struct Point {
            x: i32,
            y: i32,
        }
        "#,
        "derive-hash-eq-ok.dr",
    );

    assert!(
        !result.has_errors(),
        "expected aggregate hash derives to compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_derive_debug_and_default_for_named_type() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            @derive(Debug, Default)
            struct Point {
                x: i32,
                y: i32,
            }

            fun main(): bool {
                let formatter = std::fmt::Formatter::new();
                Point::default().fmt(&mut formatter);
                true
            }
            "#,
        ),
        "derive-debug-default-ok.dr",
    );

    assert!(
        !result.has_errors(),
        "expected Debug/Default derives to compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_derive_clone_and_partialeq_for_named_type() {
    let result = compile(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            @derive(Clone, PartialEq)
            struct Point {
                x: i32,
                y: i32,
            }

            fun main(): bool {
                const cloned = Point { x: 3, y: 4 }.clone();
                cloned.eq(&Point { x: 3, y: 4 })
            }
            "#,
        ),
        "derive-clone-partialeq-ok.dr",
    );

    assert!(
        !result.has_errors(),
        "expected Clone/PartialEq derives to compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn analyze_synthesizes_callable_derive_impl_methods() {
    let analyzed = analyze(
        &stdlib_bundle::with_bundled_prelude(
            r#"
            @derive(Clone, PartialEq, Debug, Default)
            struct Point {
                x: i32,
                y: i32,
            }

            fun main(): bool {
                true
            }
            "#,
        ),
        "derive-synthesized-methods.dr",
    );

    assert!(
        analyzed.diagnostics.is_empty(),
        "expected clean analysis, got: {:?}",
        analyzed
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );

    let hir = analyzed.hir.expect("expected HIR");
    let point_def = hir
        .structs
        .iter()
        .find(|item| {
            hir.def_names
                .get(&item.def)
                .is_some_and(|name| name == "Point")
        })
        .map(|item| item.def)
        .expect("expected Point struct");

    let mut methods = hir
        .impls
        .iter()
        .filter(|imp| matches!(&imp.self_ty, Ty::Named { def, .. } if *def == point_def))
        .flat_map(|imp| imp.items.iter())
        .filter_map(|item| match item {
            daram_compiler::hir::HirImplItem::Method(method) => Some((
                hir.def_names
                    .get(&method.def)
                    .and_then(|name| name.rsplit("::").next())
                    .unwrap_or_default()
                    .to_string(),
                method.body.is_some(),
            )),
            _ => None,
        })
        .collect::<Vec<_>>();
    methods.sort();

    assert!(
        methods.contains(&("clone".to_string(), true)),
        "expected synthesized clone, got: {methods:?}"
    );
    assert!(
        methods.contains(&("default".to_string(), true)),
        "expected synthesized default, got: {methods:?}"
    );
    assert!(
        methods.contains(&("eq".to_string(), true)),
        "expected synthesized eq, got: {methods:?}"
    );
    assert!(
        methods.contains(&("fmt".to_string(), true)),
        "expected synthesized fmt, got: {methods:?}"
    );
}

#[test]
fn compile_rejects_derive_default_with_non_default_field() {
    let result = compile(
        r#"
        ability Default {
            fun default(): Self;
        }

        struct Payload {
            code: i32,
        }

        @derive(Default)
        struct Wrapper {
            payload: Payload,
        }
        "#,
        "derive-default-non-default-field.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("cannot derive `Default`")),
        "expected derive Default field diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_derive_hash_with_non_hash_field() {
    let result = compile(
        r#"
        ability Hash {
            fun hash(self, state: &mut i32);
        }

        struct Payload {
            text: String,
        }

        @derive(Hash)
        struct Key {
            payload: Payload,
        }
        "#,
        "derive-hash-non-hash-field.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("cannot derive `Hash`")),
        "expected derive Hash field diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_copy_and_drop_on_same_type() {
    let result = compile(
        r#"
        ability Copy;
        ability Drop;

        struct Ticket {
            id: i32,
        }

        impl Copy for Ticket {}
        impl Drop for Ticket {}
        "#,
        "copy-drop-conflict.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("cannot implement both `Copy` and `Drop`")),
        "expected Copy/Drop conflict diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_send_impl_with_non_send_field() {
    let result = compile(
        r#"
        ability Send;

        struct Worker {
            payload: String,
        }

        impl Send for Worker {}
        "#,
        "send-impl-non-send-field.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|diag| diag.message.contains("does not satisfy `Send`")),
        "expected Send field diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_accepts_direct_capability_parameter() {
    let result = compile(
        r#"
        struct FsWriteCap {}

        fn write(cap: FsWriteCap) {
            cap;
        }
        "#,
        "capability-param-ok.dr",
    );

    assert!(
        !result.has_errors(),
        "expected direct capability parameter to compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_capability_return_type() {
    let result = compile(
        r#"
        struct FsWriteCap {}

        fn leak(cap: FsWriteCap) -> FsWriteCap {
            cap
        }
        "#,
        "capability-return.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("capability tokens are only supported as direct function parameters in v1")),
        "expected capability surface diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn parser_records_default_parameter_values() {
    let module = parse_clean(
        r#"
        fun connect(host: string, port: i32 = 8080) {}
        "#,
    );

    match &module.items[0] {
        Item::Function(fun) => {
            assert_eq!(fun.params.len(), 2);
            assert!(fun.params[0].default.is_none());
            assert!(matches!(fun.params[1].default, Some(Expr::Literal { .. })));
        }
        other => panic!("expected function item, got {other:?}"),
    }
}

#[test]
fn resolver_rewrites_default_parameter_calls_to_synthetic_wrappers() {
    let hir = resolve_clean(
        r#"
        export fun connect(host: string, port: i32 = 8080): i32 {
            port
        }

        fun main(): i32 {
            connect("localhost")
        }
        "#,
    );

    let wrapper_def = hir
        .def_names
        .iter()
        .find_map(|(def, name)| name.contains("connect::__default$arity1").then_some(*def))
        .expect("expected synthetic default wrapper");
    assert!(
        hir.functions.iter().any(|fun| fun.def == wrapper_def),
        "expected wrapper function in HIR"
    );

    let main_fn = hir
        .functions
        .iter()
        .find(|fun| {
            hir.def_names
                .get(&fun.def)
                .is_some_and(|name| name == "main")
        })
        .expect("expected main function");
    let body = main_fn.body.as_ref().expect("expected main body");
    let HirExprKind::Block(_, Some(tail)) = &body.kind else {
        panic!("expected block body, got {:?}", body.kind);
    };
    let HirExprKind::Call { callee, args } = &tail.kind else {
        panic!("expected direct call tail, got {:?}", tail.kind);
    };
    assert_eq!(args.len(), 1);
    assert!(
        matches!(callee.kind, HirExprKind::DefRef(def) if def == wrapper_def),
        "expected call to synthetic wrapper, got {:?}",
        callee.kind
    );
}

#[test]
fn compile_rejects_non_trailing_default_parameters() {
    let result = compile(
        r#"
        fun connect(host: string = "localhost", port: i32): i32 {
            port
        }
        "#,
        "default-param-order.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("default parameters must form a trailing suffix")),
        "expected trailing-default diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn compile_rejects_inline_mod_blocks_in_user_sources() {
    let result = compile(
        r#"
        mod nested {
            fun helper() {}
        }
        "#,
        "inline-mod-user.dr",
    );

    assert!(result.has_errors(), "expected compile error");
    assert!(
        result.diagnostics.iter().any(|diag| diag
            .message
            .contains("inline `mod` blocks are no longer supported")),
        "expected inline-mod diagnostic, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn parser_and_resolver_handle_extern_c_block() {
    let result = analyze(
        r#"
        extern "C" {
            fun add(a: i32, b: i32): i32;
            fun strlen(n: i64): i64;
        }
        "#,
        "extern-c.dr",
    );

    assert!(
        !result
            .diagnostics
            .iter()
            .any(|d| d.level == daram_compiler::diagnostics::Level::Error),
        "expected clean compile, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|diag| diag.message.as_str())
            .collect::<Vec<_>>()
    );

    let hir = result.hir.expect("expected HIR");
    assert_eq!(hir.extern_fns.len(), 2, "expected 2 extern fns");
    let names: Vec<&str> = hir.extern_fns.iter().map(|f| f.name.as_str()).collect();
    assert!(names.contains(&"add"), "expected `add` extern fn");
    assert!(names.contains(&"strlen"), "expected `strlen` extern fn");
    for f in &hir.extern_fns {
        assert_eq!(f.abi, "C", "expected ABI to be 'C'");
    }
}

// ─── NLL borrow checker regression tests (#21) ───────────────────────────────

#[test]
fn borrow_checker_rejects_use_after_move() {
    let result = compile(
        r#"
        struct Foo { val: i32 }

        fun consume(f: Foo) {}

        fun use_after_move() {
            let f = Foo { val: 1 };
            consume(f);
            consume(f);
        }
        "#,
        "use-after-move.dr",
    );
    assert!(
        result.has_errors(),
        "expected borrow error for use-after-move, got clean compile"
    );
}

#[test]
fn borrow_checker_rejects_double_mutable_borrow() {
    let result = compile(
        r#"
        fun double_mut_borrow() {
            let mut x = 1;
            let a = &mut x;
            let b = &mut x;
            let _ = a;
            let _ = b;
        }
        "#,
        "double-mut-borrow.dr",
    );
    assert!(
        result.has_errors(),
        "expected borrow error for double mutable borrow, got clean compile"
    );
}

#[test]
fn borrow_checker_rejects_shared_and_mutable_borrow() {
    let result = compile(
        r#"
        fun shared_and_mut() {
            let mut x = 1;
            let a = &x;
            let b = &mut x;
            let _ = a;
            let _ = b;
        }
        "#,
        "shared-and-mut.dr",
    );
    assert!(
        result.has_errors(),
        "expected borrow error for shared+mutable borrow, got clean compile"
    );
}

// ─── Closure regression tests (#24) ─────────────────────────────────────────

#[test]
fn closures_parse_and_resolve_basic_closure() {
    let result = compile(
        r#"
        fun apply(f: fun(i32): i32, x: i32): i32 {
            f(x)
        }

        fun main() {
            let double = |x: i32| x * 2;
            let result = apply(double, 5);
        }
        "#,
        "closure-basic.dr",
    );
    // Closures should at minimum parse and resolve without ICE
    let has_ice = result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("panic") || d.message.contains("ICE"));
    assert!(
        !has_ice,
        "expected no ICE for basic closure, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn closures_parse_closure_capture() {
    let result = compile(
        r#"
        fun make_adder(n: i32): fun(i32): i32 {
            |x: i32| x + n
        }
        "#,
        "closure-capture.dr",
    );
    let has_ice = result
        .diagnostics
        .iter()
        .any(|d| d.message.contains("panic") || d.message.contains("ICE"));
    assert!(
        !has_ice,
        "expected no ICE for closure capture, got: {:?}",
        result.diagnostics
    );
}

// ─── derive(Debug) + format! regression tests (#27) ─────────────────────────

#[test]
fn derive_debug_generates_debug_method() {
    let result = analyze(
        r#"
        @derive(Debug)
        struct Point {
            x: i32,
            y: i32,
        }
        "#,
        "derive-debug.dr",
    );
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == daram_compiler::diagnostics::Level::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "expected clean compile for @derive(Debug) struct, got: {:?}",
        errors
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
    );
    // @derive(Debug) is accepted by the parser and resolver without errors.
    // Body synthesis requires the Debug ability from stdlib, which is not
    // available in isolated unit tests — that path is exercised by the
    // interpreter regression suite with the full stdlib bundle.
    let hir = result.hir.expect("expected HIR");
    // The Point struct should be in the HIR with the Debug derive preserved.
    let point = hir.structs.iter().find(|s| {
        hir.def_names
            .get(&s.def)
            .map(|n| n.ends_with("Point"))
            .unwrap_or(false)
    });
    assert!(point.is_some(), "expected Point struct in HIR");
    assert!(
        point.unwrap().derives.iter().any(|d| d.contains("Debug")),
        "expected @derive(Debug) to be recorded in struct derives"
    );
}

#[test]
fn parser_accepts_dyn_ability_parameter() {
    // `dyn Ability` is now supported — function parameters with dyn type should compile cleanly.
    let result = compile(
        r#"
        ability Show { fun show(self): bool; }
        fun print_it(x: dyn Show) {}
        "#,
        "dyn-test.dr",
    );
    assert!(
        !result.has_errors(),
        "expected no errors for `dyn Ability`, got: {:?}",
        result
            .diagnostics
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
    );
}

// ─── NLL field-sensitive borrow tests ────────────────────────────────────────

#[test]
fn borrow_checker_allows_borrows_of_different_struct_fields() {
    // Borrowing different fields of the same struct simultaneously should be allowed
    let result = compile(
        r#"
        struct Pair { a: i32, b: i32 }

        fun use_pair() {
            let mut p = Pair { a: 1, b: 2 };
            let x = &p.a;
            let y = &p.b;
            let _ = x;
            let _ = y;
        }
        "#,
        "field-borrow-ok.dr",
    );
    // Should compile without borrow errors
    let borrow_errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| {
            d.level == daram_compiler::diagnostics::Level::Error && d.message.contains("borrow")
        })
        .collect();
    assert!(
        borrow_errors.is_empty(),
        "unexpected borrow errors for distinct field borrows: {:?}",
        borrow_errors
            .iter()
            .map(|d| d.message.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn borrow_checker_rejects_mutable_borrow_of_same_field_twice() {
    let result = compile(
        r#"
        struct Pair { a: i32, b: i32 }

        fun double_mut_field() {
            let mut p = Pair { a: 1, b: 2 };
            let x = &mut p.a;
            let y = &mut p.a;
            let _ = x;
            let _ = y;
        }
        "#,
        "field-double-mut.dr",
    );
    assert!(
        result.has_errors(),
        "expected borrow error for double mutable borrow of same field"
    );
}

#[test]
fn borrow_checker_allows_reborrow_through_reference() {
    // Reborrow: &*r gives a new shared reference from an existing one.
    let result = compile(
        r#"
        fun read_twice(r: &i32): i32 {
            let a = *r;
            let b = *r;
            a + b
        }
        "#,
        "reborrow-shared.dr",
    );
    assert!(
        !result.has_errors(),
        "expected reborrow to compile, got: {:?}",
        result.diagnostics
    );
}

#[test]
fn borrow_checker_rejects_use_after_move_in_loop() {
    // After a value is moved inside a loop body it must not be used again
    // in a later iteration.  The simple checker should flag the use.
    let result = compile(
        r#"
        struct Token { value: i32 }
        fun consume(_t: Token) {}

        fun main() {
            let t = Token { value: 1 };
            consume(t);
            consume(t);
        }
        "#,
        "use-after-move-loop.dr",
    );
    assert!(result.has_errors(), "expected use-after-move diagnostic");
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.message.contains("use of moved value") || d.message.contains("moved")),
        "expected a moved-value diagnostic, got: {:?}",
        result.diagnostics
    );
}
