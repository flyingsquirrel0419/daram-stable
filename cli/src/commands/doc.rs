//! `dr doc` — generate HTML documentation.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{terminal, workspace::find_workspace};
use daram_compiler::{
    ast::{Item, Module},
    lexer::lex,
    parser::parse,
    source::SourceMap,
    stdlib_bundle::{self, StdlibStability},
};

pub fn run(args: &[String]) -> i32 {
    let open = args.iter().any(|a| a == "--open");

    let ws = match find_workspace() {
        Ok(w) => w,
        Err(e) => {
            terminal::error(&e.to_string());
            return 1;
        }
    };
    let include_private = args.iter().any(|a| a == "--private") || ws.manifest.doc.include_private;

    let src_dir = ws.root.join("src");
    if !src_dir.exists() {
        terminal::error("`src/` directory not found");
        return 1;
    }

    let output_dir = ws.root.join(&ws.manifest.doc.output_dir);
    if let Err(e) = fs::create_dir_all(&output_dir) {
        terminal::error(&format!("failed to create doc directory: {}", e));
        return 1;
    }

    terminal::step(&format!(
        "generating documentation for `{}` v{}",
        ws.manifest.package.name, ws.manifest.package.version
    ));

    let mut source_map = SourceMap::new();
    let mut pages: Vec<DocPage> = Vec::new();

    if let Err(e) = collect_doc_pages(
        &src_dir,
        &src_dir,
        &mut source_map,
        include_private,
        &mut pages,
    ) {
        terminal::error(&format!("I/O error collecting sources: {}", e));
        return 1;
    }

    // Generate index page.
    let index_html = render_index_page(
        &ws.manifest.package.name,
        &ws.manifest.package.version,
        ws.manifest.package.description.as_deref().unwrap_or(""),
        &pages,
    );
    if let Err(e) = fs::write(output_dir.join("index.html"), &index_html) {
        terminal::error(&format!("failed to write index.html: {}", e));
        return 1;
    }

    // Generate per-module pages.
    for page in &pages {
        let page_dir = output_dir.join(&page.module_path.replace("::", "/"));
        if let Err(e) = fs::create_dir_all(&page_dir) {
            terminal::error(&format!("failed to create doc dir: {}", e));
            return 1;
        }
        let html = render_module_page(page);
        if let Err(e) = fs::write(page_dir.join("index.html"), &html) {
            terminal::error(&format!("failed to write {}: {}", page_dir.display(), e));
            return 1;
        }
    }

    // Write a basic CSS file.
    let _ = fs::write(output_dir.join("daram-doc.css"), DOC_CSS);

    terminal::success(&format!(
        "documentation written to `{}`",
        output_dir.display()
    ));

    if open {
        let index = output_dir.join("index.html");
        let _ = open_url(index.to_str().unwrap_or(""));
    }

    0
}

// ─── Doc data structures ──────────────────────────────────────────────────────

struct DocPage {
    module_path: String,
    items: Vec<DocItem>,
    file_name: String,
    module_doc: String,
    unstable: bool,
}

struct DocItem {
    kind: DocItemKind,
    name: String,
    visibility: bool,
    doc_comment: String,
    unstable: bool,
    capability_tokens: Vec<String>,
}

enum DocItemKind {
    Function { signature: String },
    Struct { fields: Vec<String> },
    Enum { variants: Vec<String> },
    Trait { methods: Vec<String> },
    Interface { methods: Vec<String> },
    Ability { supers: Vec<String> },
    TypeAlias { ty: String },
    Constant { ty: String },
}

fn collect_doc_pages(
    src_root: &Path,
    dir: &PathBuf,
    source_map: &mut SourceMap,
    include_private: bool,
    pages: &mut Vec<DocPage>,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_doc_pages(src_root, &path, source_map, include_private, pages)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("dr") {
            let src = fs::read_to_string(&path)?;
            let name = path.to_string_lossy().to_string();
            let file_id = source_map.add_file(name.clone(), src.clone());
            let tokens = lex(&src);
            let (module, _errors) = parse(file_id, &tokens);
            let module_path = module_path_from_file(src_root, &path);
            let page = extract_doc_page(&module, &module_path, &src, include_private);
            pages.push(page);
        }
    }
    Ok(())
}

fn extract_doc_page(
    module: &Module,
    module_path: &str,
    src: &str,
    include_private: bool,
) -> DocPage {
    let mut items = Vec::new();
    for item in &module.items {
        if let Some(doc_item) = extract_doc_item(item, module_path, src, include_private) {
            items.push(doc_item);
        }
    }
    let module_doc = extract_module_doc(src);
    let module_unstable = is_unstable_doc(&module_doc)
        || matches!(
            stdlib_bundle::stdlib_stability_for_path(&format!("std::{module_path}")),
            Some(StdlibStability::Unstable)
        );
    DocPage {
        module_path: module_path.to_string(),
        items,
        file_name: module_path.to_string(),
        module_doc,
        unstable: module_unstable,
    }
}

fn extract_doc_item(
    item: &Item,
    module_path: &str,
    src: &str,
    include_private: bool,
) -> Option<DocItem> {
    let module_is_unstable = matches!(
        stdlib_bundle::stdlib_stability_for_path(&format!("std::{module_path}")),
        Some(StdlibStability::Unstable)
    );
    match item {
        Item::Function(f) => {
            if !include_private && !f.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, f.span.start.0 as usize);
            let capability_tokens = extract_capability_tokens_from_fn(f);
            let params = f
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}: {}",
                        render_pattern_name(&param.pattern),
                        render_type_expr(&param.ty)
                    )
                })
                .collect::<Vec<_>>();
            let mut sig = format!("fn {}({})", f.name.name, params.join(", "));
            if let Some(ret) = &f.ret_ty {
                sig.push_str(&format!(" -> {}", render_type_expr(ret)));
            }
            Some(DocItem {
                kind: DocItemKind::Function { signature: sig },
                name: f.name.name.clone(),
                visibility: f.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens,
                doc_comment,
            })
        }
        Item::Struct(s) => {
            if !include_private && !s.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, s.span.start.0 as usize);
            let fields = match &s.kind {
                daram_compiler::ast::StructKind::Fields(fs) => fs
                    .iter()
                    .map(|f| format!("{}: {}", f.name.name, render_type_expr(&f.ty)))
                    .collect(),
                _ => Vec::new(),
            };
            Some(DocItem {
                kind: DocItemKind::Struct { fields },
                name: s.name.name.clone(),
                visibility: s.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens: if s.name.name.ends_with("Cap") {
                    vec![s.name.name.clone()]
                } else {
                    Vec::new()
                },
                doc_comment,
            })
        }
        Item::Enum(e) => {
            if !include_private && !e.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, e.span.start.0 as usize);
            let variants = e.variants.iter().map(|v| v.name.name.clone()).collect();
            Some(DocItem {
                kind: DocItemKind::Enum { variants },
                name: e.name.name.clone(),
                visibility: e.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens: Vec::new(),
                doc_comment,
            })
        }
        Item::Trait(t) => {
            if !include_private && !t.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, t.span.start.0 as usize);
            let methods = t
                .items
                .iter()
                .filter_map(|item| match item {
                    daram_compiler::ast::TraitItem::Method(method) => {
                        Some(method.name.name.clone())
                    }
                    _ => None,
                })
                .collect();
            Some(DocItem {
                kind: DocItemKind::Trait { methods },
                name: t.name.name.clone(),
                visibility: t.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens: Vec::new(),
                doc_comment,
            })
        }
        Item::Interface(interface) => {
            if !include_private && !interface.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, interface.span.start.0 as usize);
            let methods = interface
                .items
                .iter()
                .filter_map(|item| match item {
                    daram_compiler::ast::TraitItem::Method(method) => {
                        Some(method.name.name.clone())
                    }
                    _ => None,
                })
                .collect();
            Some(DocItem {
                kind: DocItemKind::Interface { methods },
                name: interface.name.name.clone(),
                visibility: interface.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens: Vec::new(),
                doc_comment,
            })
        }
        Item::Ability(ability) => {
            if !include_private && !ability.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, ability.span.start.0 as usize);
            let supers = ability
                .super_abilities
                .iter()
                .map(render_path)
                .collect::<Vec<_>>();
            Some(DocItem {
                kind: DocItemKind::Ability { supers },
                name: ability.name.name.clone(),
                visibility: ability.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens: Vec::new(),
                doc_comment,
            })
        }
        Item::TypeAlias(alias) => {
            if !include_private && !alias.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, alias.span.start.0 as usize);
            Some(DocItem {
                kind: DocItemKind::TypeAlias {
                    ty: render_type_expr(&alias.ty),
                },
                name: alias.name.name.clone(),
                visibility: alias.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens: Vec::new(),
                doc_comment,
            })
        }
        Item::Const(constant) => {
            if !include_private && !constant.visibility.is_pub {
                return None;
            }
            let doc_comment = extract_doc_comment(src, constant.span.start.0 as usize);
            Some(DocItem {
                kind: DocItemKind::Constant {
                    ty: render_type_expr(&constant.ty),
                },
                name: constant.name.name.clone(),
                visibility: constant.visibility.is_pub,
                unstable: module_is_unstable || is_unstable_doc(&doc_comment),
                capability_tokens: Vec::new(),
                doc_comment,
            })
        }
        _ => None,
    }
}

fn module_path_from_file(src_root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(src_root).unwrap_or(path);
    let mut segments = relative
        .iter()
        .map(|segment| segment.to_string_lossy().to_string())
        .collect::<Vec<_>>();
    if segments.last().is_some_and(|segment| segment == "mod.dr") {
        segments.pop();
    } else if let Some(last) = segments.last_mut() {
        if let Some(stripped) = last.strip_suffix(".dr") {
            *last = stripped.to_string();
        }
    }
    if segments.is_empty() {
        "crate".to_string()
    } else {
        segments.join("::")
    }
}

// ─── HTML rendering ───────────────────────────────────────────────────────────

fn render_index_page(pkg: &str, version: &str, description: &str, pages: &[DocPage]) -> String {
    let mut html = page_head(&format!("{} — Daram Documentation", pkg));
    html.push_str(&format!(
        "<h1>{} <span class=\"version\">v{}</span></h1>\n<ul class=\"modules\">\n",
        html_escape(pkg),
        html_escape(version)
    ));
    if !description.trim().is_empty() {
        html.push_str(&format!(
            "<p class=\"package-doc\">{}</p>\n",
            html_escape(description.trim())
        ));
    }
    for page in pages {
        let item_count = page.items.len();
        html.push_str(&format!(
            "<li><a href=\"{}/index.html\">{}</a>{} ({} item(s))</li>\n",
            html_escape(&page.module_path.replace("::", "/")),
            html_escape(&page.module_path),
            if page.unstable {
                " <span class=\"badge badge-unstable\">unstable</span>"
            } else {
                ""
            },
            item_count,
        ));
    }
    html.push_str("</ul>\n");
    html.push_str(&page_foot());
    html
}

fn render_module_page(page: &DocPage) -> String {
    let mut html = page_head(&format!("{} — Daram Documentation", page.module_path));
    html.push_str(&format!(
        "<h1>{}{}</h1>\n",
        html_escape(&page.module_path),
        if page.unstable {
            " <span class=\"badge badge-unstable\">unstable</span>"
        } else {
            ""
        }
    ));
    html.push_str(&format!(
        "<p class=\"module-file\">Source: <code>{}</code></p>\n",
        html_escape(&page.file_name)
    ));
    if !page.module_doc.is_empty() {
        html.push_str(&format!(
            "<p class=\"module-doc\">{}</p>\n",
            html_escape(&page.module_doc)
        ));
    }

    for item in &page.items {
        let visibility = if item.visibility { "pub " } else { "" };
        html.push_str("<section class=\"item\">\n");
        match &item.kind {
            DocItemKind::Function { signature } => {
                html.push_str(&format!(
                    "<h3 class=\"fn\"><code>{}</code></h3>\n",
                    html_escape(&format!("{}{}", visibility, signature))
                ));
            }
            DocItemKind::Struct { fields } => {
                html.push_str(&format!(
                    "<h3 class=\"struct\">{}struct {}</h3>\n",
                    html_escape(visibility),
                    html_escape(&item.name)
                ));
                if !fields.is_empty() {
                    html.push_str("<ul>\n");
                    for f in fields {
                        html.push_str(&format!("<li><code>{}</code></li>\n", html_escape(f)));
                    }
                    html.push_str("</ul>\n");
                }
            }
            DocItemKind::Enum { variants } => {
                html.push_str(&format!(
                    "<h3 class=\"enum\">{}enum {}</h3>\n",
                    html_escape(visibility),
                    html_escape(&item.name)
                ));
                html.push_str("<ul>\n");
                for v in variants {
                    html.push_str(&format!("<li><code>{}</code></li>\n", html_escape(v)));
                }
                html.push_str("</ul>\n");
            }
            DocItemKind::Trait { methods } => {
                html.push_str(&format!(
                    "<h3 class=\"trait\">{}trait {}</h3>\n",
                    html_escape(visibility),
                    html_escape(&item.name)
                ));
                if !methods.is_empty() {
                    html.push_str("<ul>\n");
                    for method in methods {
                        html.push_str(&format!("<li><code>{}</code></li>\n", html_escape(method)));
                    }
                    html.push_str("</ul>\n");
                }
            }
            DocItemKind::Interface { methods } => {
                html.push_str(&format!(
                    "<h3 class=\"trait\">{}interface {}</h3>\n",
                    html_escape(visibility),
                    html_escape(&item.name)
                ));
                if !methods.is_empty() {
                    html.push_str("<ul>\n");
                    for method in methods {
                        html.push_str(&format!("<li><code>{}</code></li>\n", html_escape(method)));
                    }
                    html.push_str("</ul>\n");
                }
            }
            DocItemKind::Ability { supers } => {
                html.push_str(&format!(
                    "<h3 class=\"trait\">{}ability {}</h3>\n",
                    html_escape(visibility),
                    html_escape(&item.name)
                ));
                if !supers.is_empty() {
                    html.push_str(&format!(
                        "<p class=\"meta\">super abilities: <code>{}</code></p>\n",
                        html_escape(&supers.join(", "))
                    ));
                }
            }
            DocItemKind::TypeAlias { ty } => {
                html.push_str(&format!(
                    "<h3 class=\"type\">{}type {} = <code>{}</code></h3>\n",
                    html_escape(visibility),
                    html_escape(&item.name),
                    html_escape(ty)
                ));
            }
            DocItemKind::Constant { ty } => {
                html.push_str(&format!(
                    "<h3 class=\"const\">{}const {}: <code>{}</code></h3>\n",
                    html_escape(visibility),
                    html_escape(&item.name),
                    html_escape(ty)
                ));
            }
        }
        if item.unstable {
            html.push_str(
                "<p class=\"meta\"><span class=\"badge badge-unstable\">unstable</span></p>\n",
            );
        }
        if !item.capability_tokens.is_empty() {
            html.push_str(&format!(
                "<p class=\"meta\">capability tokens: <code>{}</code></p>\n",
                html_escape(&item.capability_tokens.join(", "))
            ));
        }
        if !item.doc_comment.is_empty() {
            html.push_str(&format!("<p>{}</p>\n", html_escape(&item.doc_comment)));
        }
        html.push_str("</section>\n");
    }

    html.push_str(&page_foot());
    html
}

fn page_head(title: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>{}</title>
<link rel="stylesheet" href="/daram-doc.css">
</head>
<body>
<nav class="top-nav"><a href="/index.html">Daram Docs</a></nav>
<main>
"#,
        html_escape(title)
    )
}

fn page_foot() -> &'static str {
    "</main>\n<footer>Generated by <code>dr doc</code></footer>\n</body>\n</html>\n"
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn extract_module_doc(src: &str) -> String {
    let mut docs = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(doc) = trimmed.strip_prefix("///") {
            docs.push(doc.trim().to_string());
            continue;
        }
        if trimmed.is_empty() {
            if docs.is_empty() {
                continue;
            }
            break;
        }
        break;
    }
    docs.join("\n")
}

fn extract_doc_comment(src: &str, start_byte: usize) -> String {
    let line_starts = std::iter::once(0usize)
        .chain(src.match_indices('\n').map(|(idx, _)| idx + 1))
        .collect::<Vec<_>>();
    let item_line_index = match line_starts.binary_search(&start_byte) {
        Ok(index) => index,
        Err(index) => index.saturating_sub(1),
    };
    let lines = src.lines().collect::<Vec<_>>();
    if item_line_index == 0 || item_line_index > lines.len() {
        return String::new();
    }

    let mut docs = Vec::new();
    let mut cursor = item_line_index;
    while cursor > 0 {
        cursor -= 1;
        let trimmed = lines[cursor].trim_start();
        if let Some(doc) = trimmed.strip_prefix("///") {
            docs.push(doc.trim().to_string());
            continue;
        }
        if trimmed.is_empty() && docs.is_empty() {
            continue;
        }
        break;
    }
    docs.reverse();
    docs.join("\n")
}

fn is_unstable_doc(doc: &str) -> bool {
    let lower = doc.to_ascii_lowercase();
    lower.contains("experimental") || lower.contains("unstable")
}

fn render_path(path: &daram_compiler::ast::Path) -> String {
    path.segments
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join("::")
}

fn render_type_expr(ty: &daram_compiler::ast::TypeExpr) -> String {
    use daram_compiler::ast::TypeExpr;

    match ty {
        TypeExpr::Named { path, generics, .. } => {
            let mut rendered = render_path(path);
            if !generics.is_empty() {
                rendered.push('<');
                rendered.push_str(
                    &generics
                        .iter()
                        .map(render_type_expr)
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                rendered.push('>');
            }
            rendered
        }
        TypeExpr::Ref { mutable, inner, .. } => {
            format!(
                "&{}{}",
                if *mutable { "mut " } else { "" },
                render_type_expr(inner)
            )
        }
        TypeExpr::Tuple { elems, .. } => format!(
            "({})",
            elems
                .iter()
                .map(render_type_expr)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeExpr::Array { elem, len, .. } => format!("[{}; {:?}]", render_type_expr(elem), len),
        TypeExpr::Slice { elem, .. } => format!("[{}]", render_type_expr(elem)),
        TypeExpr::FnPtr { params, ret, .. } => {
            let mut rendered = format!(
                "fn({})",
                params
                    .iter()
                    .map(render_type_expr)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if let Some(ret) = ret {
                rendered.push_str(&format!(" -> {}", render_type_expr(ret)));
            }
            rendered
        }
        TypeExpr::Never { .. } => "!".to_string(),
        TypeExpr::Infer { .. } => "_".to_string(),
        TypeExpr::SelfType { .. } => "Self".to_string(),
        TypeExpr::DynTrait { ability, .. } => {
            format!(
                "dyn {}",
                ability
                    .segments
                    .iter()
                    .map(|s| s.name.as_str())
                    .collect::<Vec<_>>()
                    .join("::")
            )
        }
    }
}

fn render_pattern_name(pattern: &daram_compiler::ast::Pattern) -> String {
    use daram_compiler::ast::Pattern;

    match pattern {
        Pattern::Ident { name, .. } => name.name.clone(),
        Pattern::Wildcard { .. } => "_".to_string(),
        Pattern::Ref { inner, .. } => render_pattern_name(inner),
        _ => "arg".to_string(),
    }
}

fn extract_capability_tokens_from_fn(f: &daram_compiler::ast::FnDef) -> Vec<String> {
    let mut caps = Vec::new();
    for param in &f.params {
        collect_capability_tokens_from_type(&param.ty, &mut caps);
    }
    caps.sort();
    caps.dedup();
    caps
}

fn collect_capability_tokens_from_type(ty: &daram_compiler::ast::TypeExpr, caps: &mut Vec<String>) {
    use daram_compiler::ast::TypeExpr;

    match ty {
        TypeExpr::Named { path, generics, .. } => {
            if let Some(last) = path.segments.last() {
                if last.name.ends_with("Cap") {
                    caps.push(last.name.clone());
                }
            }
            for generic in generics {
                collect_capability_tokens_from_type(generic, caps);
            }
        }
        TypeExpr::Ref { inner, .. } | TypeExpr::Slice { elem: inner, .. } => {
            collect_capability_tokens_from_type(inner, caps);
        }
        TypeExpr::Array { elem, .. } => collect_capability_tokens_from_type(elem, caps),
        TypeExpr::Tuple { elems, .. } => {
            for elem in elems {
                collect_capability_tokens_from_type(elem, caps);
            }
        }
        TypeExpr::FnPtr { params, ret, .. } => {
            for param in params {
                collect_capability_tokens_from_type(param, caps);
            }
            if let Some(ret) = ret {
                collect_capability_tokens_from_type(ret, caps);
            }
        }
        TypeExpr::Never { .. }
        | TypeExpr::Infer { .. }
        | TypeExpr::SelfType { .. }
        | TypeExpr::DynTrait { .. } => {}
    }
}

fn open_url(path: &str) {
    let _ = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(path).spawn()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("cmd")
            .args(["/c", "start", path])
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(path).spawn()
    };
}

const DOC_CSS: &str = r#"
:root { --bg: #0f1117; --fg: #e2e8f0; --accent: #14b8a6; --border: rgba(255,255,255,0.1); }
* { box-sizing: border-box; margin: 0; padding: 0; }
body { background: var(--bg); color: var(--fg); font-family: system-ui, sans-serif; line-height: 1.6; }
nav.top-nav { padding: 0.75rem 1.5rem; border-bottom: 1px solid var(--border); }
nav.top-nav a { color: var(--accent); text-decoration: none; font-weight: 600; }
main { max-width: 900px; margin: 0 auto; padding: 2rem 1.5rem; }
h1 { font-size: 2rem; margin-bottom: 1rem; }
h3 { margin: 1.25rem 0 0.5rem; }
h3.fn { color: var(--accent); }
h3.struct, h3.enum, h3.trait, h3.type, h3.const { color: #a78bfa; }
p.package-doc, p.module-doc, p.meta { margin-bottom: 1rem; }
p.module-file { margin-bottom: 1rem; color: rgba(255,255,255,0.6); }
code { background: rgba(255,255,255,0.06); padding: 0.1em 0.35em; border-radius: 4px; font-family: 'JetBrains Mono', monospace; font-size: 0.9em; }
ul.modules { list-style: none; }
ul.modules li { padding: 0.35rem 0; border-bottom: 1px solid var(--border); }
ul.modules a { color: var(--accent); text-decoration: none; }
section.item { border: 1px solid var(--border); border-radius: 8px; padding: 1rem; margin-bottom: 1rem; }
.version { font-size: 1rem; color: rgba(255,255,255,0.4); }
.badge { display: inline-block; padding: 0.15rem 0.45rem; border-radius: 999px; font-size: 0.75rem; vertical-align: middle; }
.badge-unstable { background: rgba(245, 158, 11, 0.15); color: #fbbf24; border: 1px solid rgba(245, 158, 11, 0.35); }
footer { text-align: center; padding: 2rem; color: rgba(255,255,255,0.3); font-size: 0.85rem; }
"#;
