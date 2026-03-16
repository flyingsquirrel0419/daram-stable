use super::*;

pub(super) fn analyze_source(source: &str, file_name: &str) -> AnalysisResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut session = Session::new();
        let (ast, mut diagnostics) = parse_bundle_source(&mut session, source, file_name);

        let Some(ast) = ast else {
            return AnalysisResult {
                session,
                ast: None,
                hir: None,
                diagnostics,
            };
        };

        if has_errors(&diagnostics) {
            return AnalysisResult {
                session,
                ast: Some(ast),
                hir: None,
                diagnostics,
            };
        }

        let (mut hir, resolve_errs) = name_resolution::resolve(ast.file, &ast);
        diagnostics.extend(resolve_errs);

        if has_errors(&diagnostics) {
            return AnalysisResult {
                session,
                ast: Some(ast),
                hir: None,
                diagnostics,
            };
        }

        let type_errs = type_checker::check_and_prepare(ast.file, &mut hir);
        diagnostics.extend(type_errs);

        AnalysisResult {
            session,
            ast: Some(ast),
            hir: Some(hir),
            diagnostics,
        }
    })) {
        Ok(result) => result,
        Err(_) => AnalysisResult {
            session: Session::new(),
            ast: None,
            hir: None,
            diagnostics: vec![diagnostics::Diagnostic::error(PANIC_RECOVERY_MESSAGE)],
        },
    }
}

fn has_errors(diagnostics: &[diagnostics::Diagnostic]) -> bool {
    diagnostics
        .iter()
        .any(|diag| diag.level == diagnostics::Level::Error)
}

fn bundle_module_segments(path: &str) -> Vec<String> {
    let normalized = path.replace('\\', "/");
    let mut segments = normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return Vec::new();
    }

    let last = segments.pop().unwrap_or_default();
    match last.as_str() {
        "main.dr" | "lib.dr" | "mod.dr" => {}
        other => segments.push(other.trim_end_matches(".dr").to_string()),
    }
    segments
}

fn insert_bundle_module(
    root: &mut AstBundleNode,
    path: &str,
    file: source::FileId,
    module: ast::Module,
    diagnostics: &mut Vec<diagnostics::Diagnostic>,
) {
    let segments = bundle_module_segments(path);
    let mut node = root;
    for segment in segments {
        if !node.children.contains_key(&segment) {
            node.child_order.push(segment.clone());
        }
        node = node.children.entry(segment).or_default();
    }
    if let Some(existing) = &node.items {
        let span = existing.first().map(ast::Item::span).unwrap_or(module.span);
        diagnostics.push(
            diagnostics::Diagnostic::error(format!(
                "multiple source files map to module `{}`",
                path
            ))
            .with_span(span),
        );
        return;
    }
    node.file = Some(file);
    node.items = Some(module.items);
}

fn build_ast_module(node: &AstBundleNode, default_file: source::FileId) -> ast::Module {
    let file = node.file.unwrap_or(default_file);
    let mut items = Vec::new();

    let mut seen = HashSet::new();
    for name in &node.child_order {
        let Some(child) = node.children.get(name) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        let child_module = build_ast_module(child, file);
        let span = child_module.span;
        items.push(ast::Item::Module(ast::ModuleDef {
            visibility: ast::Visibility::private(),
            name: ast::Ident::new(name.clone(), span),
            body: Some(child_module.items),
            span,
        }));
    }

    if let Some(source_items) = &node.items {
        items.extend(source_items.clone());
    }

    let span = node
        .items
        .as_ref()
        .and_then(|source_items| {
            source_items
                .iter()
                .map(ast::Item::span)
                .reduce(|lhs, rhs| lhs.merge(rhs))
        })
        .or_else(|| items.first().map(ast::Item::span))
        .unwrap_or_else(|| source::Span::new(file, 0, 0));

    ast::Module { file, items, span }
}

fn parse_bundle_source(
    session: &mut Session,
    source: &str,
    file_name: &str,
) -> (Option<ast::Module>, Vec<diagnostics::Diagnostic>) {
    let Some(files) = stdlib_bundle::decode_source_bundle(source) else {
        let file_id = session
            .source_map
            .add_file(file_name.to_string(), source.to_string());
        let (tokens, lex_errs) = lexer::lex_with_errors(source);
        let (ast, parse_errs) = parser::parse(file_id, &tokens);
        let mut diagnostics = lex_errs
            .into_iter()
            .map(|err| diagnostics::Diagnostic::error(err.message))
            .collect::<Vec<_>>();
        diagnostics.extend(legacy_syntax_warnings(file_name, file_id, &tokens));
        diagnostics.extend(parse_errs);
        return (Some(ast), diagnostics);
    };

    let mut diagnostics = Vec::new();
    let mut root = AstBundleNode::default();
    let mut default_file = None;

    for (path, file_source) in files {
        let file_id = session
            .source_map
            .add_file(path.clone(), file_source.clone());
        if default_file.is_none() {
            default_file = Some(file_id);
        }
        let (tokens, lex_errs) = lexer::lex_with_errors(&file_source);
        let (ast, parse_errs) = parser::parse(file_id, &tokens);
        diagnostics.extend(
            lex_errs
                .into_iter()
                .map(|err| diagnostics::Diagnostic::error(err.message)),
        );
        diagnostics.extend(legacy_syntax_warnings(&path, file_id, &tokens));
        diagnostics.extend(parse_errs);
        insert_bundle_module(&mut root, &path, file_id, ast, &mut diagnostics);
    }

    (
        default_file.map(|file| build_ast_module(&root, file)),
        diagnostics,
    )
}

fn legacy_syntax_warnings(
    file_name: &str,
    file_id: source::FileId,
    tokens: &[lexer::Token],
) -> Vec<diagnostics::Diagnostic> {
    if file_name.starts_with("std/") {
        return Vec::new();
    }

    tokens
        .iter()
        .filter_map(|token| {
            let span = source::Span::new(file_id, token.start.0, token.end.0);
            let (message, note) = match &token.kind {
                lexer::TokenKind::KwFn => (
                    "`fn` is deprecated, use `fun` instead",
                    "replace `fn name(...) -> T` with `fun name(...): T`",
                ),
                lexer::TokenKind::KwPub => (
                    "`pub` is deprecated, use `export` instead",
                    "replace `pub` with `export` on public declarations",
                ),
                lexer::TokenKind::KwImpl => (
                    "`impl` is deprecated, use `extend` instead",
                    "replace `impl Foo` with `extend Foo`, and `impl Trait for Foo` with `extend Foo implements Trait`",
                ),
                lexer::TokenKind::KwUse => (
                    "`use` is deprecated, use `import ... from ...` instead",
                    "replace `use foo::bar` with `import { bar } from \"foo\"`",
                ),
                lexer::TokenKind::Arrow => (
                    "`->` is deprecated in type positions, use `:` instead",
                    "replace `fn name(...) -> T` with `fun name(...): T` and `fun(A) -> B` with `fun(A): B`",
                ),
                _ => return None,
            };
            Some(
                diagnostics::Diagnostic::warning(message)
                    .with_span(span)
                    .with_note(note),
            )
        })
        .collect()
}
