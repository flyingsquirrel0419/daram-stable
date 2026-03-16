//! `dr lsp` — minimal Language Server Protocol support over stdio.

use crate::terminal;
use daram_compiler::{
    analyze,
    ast::{Item, TraitItem},
    diagnostics::{Diagnostic, Level},
    hir::HirModule,
    lexer, parser,
    source::{FileId, SourceMap, Span},
    stdlib_bundle, Session,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use url::Url;

pub fn run(args: &[String]) -> i32 {
    if !args.is_empty() && args != ["--stdio"] {
        terminal::error("usage: dr lsp [--stdio]");
        return 1;
    }

    if let Err(error) = run_stdio() {
        terminal::error(&format!("lsp server failed: {error}"));
        return 1;
    }
    0
}

fn run_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut documents = HashMap::<String, String>::new();
    let mut shutdown_requested = false;

    while let Some(message) = read_message(&mut reader)? {
        let method = message.get("method").and_then(Value::as_str);
        match method {
            Some("initialize") => {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "capabilities": {
                                "textDocumentSync": 1,
                                "documentSymbolProvider": true,
                                "hoverProvider": true,
                                "completionProvider": {
                                    "triggerCharacters": [".", ":", " "]
                                }
                            },
                            "serverInfo": {
                                "name": "dr",
                                "version": env!("CARGO_PKG_VERSION"),
                            }
                        }
                    }),
                )?;
            }
            Some("initialized") => {}
            Some("shutdown") => {
                shutdown_requested = true;
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": Value::Null,
                    }),
                )?;
            }
            Some("exit") => {
                return Ok(());
            }
            Some("textDocument/didOpen") => {
                if let Some((uri, text)) = did_open_payload(&message) {
                    documents.insert(uri.clone(), text.clone());
                    publish_diagnostics(&mut writer, &uri, &text)?;
                }
            }
            Some("textDocument/didChange") => {
                if let Some((uri, text)) = did_change_payload(&message) {
                    documents.insert(uri.clone(), text.clone());
                    publish_diagnostics(&mut writer, &uri, &text)?;
                }
            }
            Some("textDocument/didClose") => {
                if let Some(uri) = did_close_uri(&message) {
                    documents.remove(&uri);
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "textDocument/publishDiagnostics",
                            "params": {
                                "uri": uri,
                                "diagnostics": [],
                            }
                        }),
                    )?;
                }
            }
            Some("textDocument/documentSymbol") => {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let uri = message
                    .get("params")
                    .and_then(|params| params.get("textDocument"))
                    .and_then(|doc| doc.get("uri"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let result = uri
                    .as_ref()
                    .and_then(|uri| documents.get(uri).map(|text| (uri, text)))
                    .map(|(uri, text)| document_symbols(text, &uri_to_file_name(uri)))
                    .unwrap_or_default();
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result,
                    }),
                )?;
            }
            Some("textDocument/hover") => {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let result = hover_payload(&message).and_then(|(uri, line, character)| {
                    let text = documents.get(&uri)?;
                    hover_result(text, &uri_to_file_name(&uri), line, character)
                });
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result.unwrap_or(Value::Null),
                    }),
                )?;
            }
            Some("textDocument/completion") => {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let items = completion_uri(&message)
                    .and_then(|uri| {
                        let text = documents.get(&uri)?;
                        Some(completion_items(text, &uri_to_file_name(&uri)))
                    })
                    .unwrap_or_default();
                write_message(
                    &mut writer,
                    &json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": items,
                    }),
                )?;
            }
            Some(_) | None => {
                if message.get("id").is_some() {
                    write_message(
                        &mut writer,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": message.get("id").cloned().unwrap_or(Value::Null),
                            "result": Value::Null,
                        }),
                    )?;
                }
            }
        }

        if shutdown_requested {
            writer.flush()?;
        }
    }

    Ok(())
}

fn did_open_payload(message: &Value) -> Option<(String, String)> {
    let doc = message.get("params")?.get("textDocument")?;
    Some((
        doc.get("uri")?.as_str()?.to_string(),
        doc.get("text")?.as_str()?.to_string(),
    ))
}

fn did_change_payload(message: &Value) -> Option<(String, String)> {
    let params = message.get("params")?;
    let uri = params
        .get("textDocument")?
        .get("uri")?
        .as_str()?
        .to_string();
    let changes = params.get("contentChanges")?.as_array()?;
    let text = changes.last()?.get("text")?.as_str()?.to_string();
    Some((uri, text))
}

fn did_close_uri(message: &Value) -> Option<String> {
    Some(
        message
            .get("params")?
            .get("textDocument")?
            .get("uri")?
            .as_str()?
            .to_string(),
    )
}

fn publish_diagnostics(writer: &mut impl Write, uri: &str, text: &str) -> io::Result<()> {
    let file_name = uri_to_file_name(uri);
    let result = analyze(&stdlib_bundle::with_bundled_prelude(text), &file_name);
    let diagnostics =
        lsp_diagnostics_for_file(&result.session.source_map, &result.diagnostics, &file_name);
    write_message(
        writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": uri,
                "diagnostics": diagnostics,
            }
        }),
    )
}

fn hover_payload(message: &Value) -> Option<(String, u32, u32)> {
    let params = message.get("params")?;
    let uri = params
        .get("textDocument")?
        .get("uri")?
        .as_str()?
        .to_string();
    let position = params.get("position")?;
    let line = position.get("line")?.as_u64()? as u32;
    let character = position.get("character")?.as_u64()? as u32;
    Some((uri, line, character))
}

fn hover_result(source: &str, file_name: &str, line: u32, character: u32) -> Option<Value> {
    let bundled = stdlib_bundle::with_bundled_prelude(source);
    let result = analyze(&bundled, file_name);
    let hir = result.hir?;

    // Find byte offset for the requested line/character
    let offset = line_col_to_offset(source, line, character)?;

    // Find the identifier at that offset in the original source
    let ident = ident_at_offset(source, offset)?;

    // Look up the name in HIR def_names
    let description = lookup_hover_description(&hir, &ident);
    let contents = description.unwrap_or_else(|| format!("`{ident}`"));

    Some(json!({
        "contents": {
            "kind": "markdown",
            "value": contents,
        }
    }))
}

fn completion_uri(message: &Value) -> Option<String> {
    Some(
        message
            .get("params")?
            .get("textDocument")?
            .get("uri")?
            .as_str()?
            .to_string(),
    )
}

fn completion_items(source: &str, file_name: &str) -> Value {
    let bundled = stdlib_bundle::with_bundled_prelude(source);
    let result = analyze(&bundled, file_name);
    let items = if let Some(hir) = result.hir {
        collect_completion_items(&hir)
    } else {
        Vec::new()
    };
    json!(items)
}

fn collect_completion_items(hir: &HirModule) -> Vec<Value> {
    let mut items = Vec::new();
    for f in &hir.functions {
        if let Some(name) = hir
            .def_names
            .get(&f.def)
            .and_then(|n| n.rsplit("::").next())
            .map(str::to_string)
        {
            if !name.starts_with("__") {
                items.push(json!({ "label": name, "kind": 3 })); // 3 = Function
            }
        }
    }
    for s in &hir.structs {
        if let Some(name) = hir
            .def_names
            .get(&s.def)
            .and_then(|n| n.rsplit("::").next())
            .map(str::to_string)
        {
            items.push(json!({ "label": name, "kind": 7 })); // 7 = Class/Struct
        }
    }
    for e in &hir.enums {
        if let Some(name) = hir
            .def_names
            .get(&e.def)
            .and_then(|n| n.rsplit("::").next())
            .map(str::to_string)
        {
            items.push(json!({ "label": name, "kind": 13 })); // 13 = Enum
        }
    }
    for c in &hir.consts {
        if let Some(name) = hir
            .def_names
            .get(&c.def)
            .and_then(|n| n.rsplit("::").next())
            .map(str::to_string)
        {
            items.push(json!({ "label": name, "kind": 21 })); // 21 = Constant
        }
    }
    for ext in &hir.extern_fns {
        items.push(json!({ "label": ext.name.clone(), "kind": 3 }));
    }
    items
}

fn lookup_hover_description(hir: &HirModule, ident: &str) -> Option<String> {
    // Search functions
    for f in &hir.functions {
        let full = hir.def_names.get(&f.def)?;
        let short = full.rsplit("::").next().unwrap_or(full);
        if short == ident {
            let param_tys: Vec<String> = f.params.iter().map(|p| format_ty(&p.ty)).collect();
            let ret = format_ty(&f.ret_ty);
            return Some(format!(
                "```\nfun {ident}({}): {ret}\n```",
                param_tys.join(", ")
            ));
        }
    }
    // Search structs
    for s in &hir.structs {
        let full = hir.def_names.get(&s.def)?;
        let short = full.rsplit("::").next().unwrap_or(full);
        if short == ident {
            return Some(format!("```\nstruct {ident}\n```"));
        }
    }
    // Search enums
    for e in &hir.enums {
        let full = hir.def_names.get(&e.def)?;
        let short = full.rsplit("::").next().unwrap_or(full);
        if short == ident {
            return Some(format!("```\nenum {ident}\n```"));
        }
    }
    // Search consts
    for c in &hir.consts {
        let full = hir.def_names.get(&c.def)?;
        let short = full.rsplit("::").next().unwrap_or(full);
        if short == ident {
            return Some(format!("```\nconst {ident}: {}\n```", format_ty(&c.ty)));
        }
    }
    None
}

fn format_ty(ty: &daram_compiler::hir::Ty) -> String {
    use daram_compiler::hir::Ty;
    match ty {
        Ty::Bool => "bool".to_string(),
        Ty::Int(size) => format!("{size:?}").to_lowercase(),
        Ty::Float(size) => format!("{size:?}").to_lowercase(),
        Ty::Str => "str".to_string(),
        Ty::Unit => "()".to_string(),
        Ty::Never => "!".to_string(),
        Ty::Named { def, args } => {
            let _ = (def, args);
            "?".to_string()
        }
        _ => "?".to_string(),
    }
}

fn line_col_to_offset(source: &str, line: u32, character: u32) -> Option<usize> {
    let mut current_line = 0u32;
    let mut offset = 0usize;
    for (i, ch) in source.char_indices() {
        if current_line == line {
            let col_offset = offset + character as usize;
            return if col_offset <= source.len() {
                Some(col_offset)
            } else {
                None
            };
        }
        if ch == '\n' {
            current_line += 1;
        }
        offset = i + ch.len_utf8();
    }
    if current_line == line {
        Some((offset + character as usize).min(source.len()))
    } else {
        None
    }
}

fn ident_at_offset(source: &str, offset: usize) -> Option<String> {
    let bytes = source.as_bytes();
    if offset >= bytes.len() {
        return None;
    }
    let is_ident_char = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    if !is_ident_char(bytes[offset]) {
        return None;
    }
    // Scan backward to start of identifier
    let mut start = offset;
    while start > 0 && is_ident_char(bytes[start - 1]) {
        start -= 1;
    }
    // Scan forward to end of identifier
    let mut end = offset;
    while end < bytes.len() && is_ident_char(bytes[end]) {
        end += 1;
    }
    Some(source[start..end].to_string())
}

fn read_message(reader: &mut impl BufRead) -> io::Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                let parsed = value.trim().parse::<usize>().map_err(|error| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("invalid Content-Length header: {error}"),
                    )
                })?;
                content_length = Some(parsed);
            }
        }
    }

    let content_length = content_length.ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidData, "missing Content-Length header")
    })?;
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_message(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()
}

fn uri_to_file_name(uri: &str) -> String {
    Url::parse(uri)
        .ok()
        .and_then(|url| {
            if url.scheme() == "file" {
                url.to_file_path()
                    .ok()
                    .map(|path| path.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .unwrap_or_else(|| uri.to_string())
}

fn lsp_diagnostics_for_file(
    source_map: &SourceMap,
    diagnostics: &[Diagnostic],
    file_name: &str,
) -> Vec<Value> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic_to_lsp(source_map, diagnostic, file_name))
        .collect()
}

fn diagnostic_to_lsp(
    source_map: &SourceMap,
    diagnostic: &Diagnostic,
    file_name: &str,
) -> Option<Value> {
    let span = diagnostic.primary_span?;
    let file = source_map.try_get(span.file)?;
    if file.name != file_name {
        return None;
    }
    let (start_line, start_col) = file.line_col(span.start);
    let (mut end_line, mut end_col) = file.line_col(span.end);
    if span.start == span.end {
        end_line = start_line;
        end_col = start_col.saturating_add(1);
    }
    Some(json!({
        "range": {
            "start": {
                "line": start_line.saturating_sub(1),
                "character": start_col.saturating_sub(1),
            },
            "end": {
                "line": end_line.saturating_sub(1),
                "character": end_col.saturating_sub(1),
            }
        },
        "severity": lsp_severity(diagnostic.level),
        "source": "dr",
        "message": diagnostic.message,
    }))
}

fn lsp_severity(level: Level) -> u32 {
    match level {
        Level::Error => 1,
        Level::Warning => 2,
        Level::Note => 3,
    }
}

fn document_symbols(source: &str, file_name: &str) -> Vec<Value> {
    let mut session = Session::new();
    let file = session
        .source_map
        .add_file(file_name.to_string(), source.to_string());
    let (tokens, _) = lexer::lex_with_errors(source);
    let (module, _) = parser::parse(file, &tokens);
    module
        .items
        .iter()
        .filter_map(|item| document_symbol_for_item(&session.source_map, file, item))
        .collect()
}

fn document_symbol_for_item(source_map: &SourceMap, file: FileId, item: &Item) -> Option<Value> {
    let (name, kind, span, children) = match item {
        Item::Function(item) => (&item.name.name, 12, item.span, Vec::new()),
        Item::Struct(item) => (&item.name.name, 23, item.span, Vec::new()),
        Item::Enum(item) => (&item.name.name, 10, item.span, Vec::new()),
        Item::Trait(item) => (
            &item.name.name,
            11,
            item.span,
            item.items
                .iter()
                .filter_map(|child| document_symbol_for_trait_item(source_map, file, child))
                .collect(),
        ),
        Item::Interface(item) => (
            &item.name.name,
            11,
            item.span,
            item.items
                .iter()
                .filter_map(|child| document_symbol_for_trait_item(source_map, file, child))
                .collect(),
        ),
        Item::Impl(_) => return None,
        Item::TypeAlias(item) => (&item.name.name, 26, item.span, Vec::new()),
        Item::Ability(item) => (
            &item.name.name,
            11,
            item.span,
            item.items
                .iter()
                .filter_map(|child| document_symbol_for_trait_item(source_map, file, child))
                .collect(),
        ),
        Item::Use(_, _) => return None,
        Item::Module(item) => (
            &item.name.name,
            2,
            item.span,
            item.body
                .as_ref()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|child| document_symbol_for_item(source_map, file, child))
                        .collect()
                })
                .unwrap_or_default(),
        ),
        Item::Const(item) => (&item.name.name, 14, item.span, Vec::new()),
        Item::Static(item) => (&item.name.name, 13, item.span, Vec::new()),
        Item::ExternBlock(_) => return None,
    };
    Some(document_symbol_json(
        source_map, file, name, kind, span, children,
    ))
}

fn document_symbol_for_trait_item(
    source_map: &SourceMap,
    file: FileId,
    item: &TraitItem,
) -> Option<Value> {
    match item {
        TraitItem::Method(item) => Some(document_symbol_json(
            source_map,
            file,
            &item.name.name,
            6,
            item.span,
            Vec::new(),
        )),
        TraitItem::TypeAssoc { name, span, .. } => Some(document_symbol_json(
            source_map,
            file,
            &name.name,
            26,
            *span,
            Vec::new(),
        )),
        TraitItem::Const { name, span, .. } => Some(document_symbol_json(
            source_map,
            file,
            &name.name,
            14,
            *span,
            Vec::new(),
        )),
    }
}

fn document_symbol_json(
    source_map: &SourceMap,
    file: FileId,
    name: &str,
    kind: u32,
    span: Span,
    children: Vec<Value>,
) -> Value {
    json!({
        "name": name,
        "kind": kind,
        "range": span_to_lsp_range(source_map, file, span),
        "selectionRange": span_to_lsp_range(source_map, file, span),
        "children": children,
    })
}

fn span_to_lsp_range(source_map: &SourceMap, file: FileId, span: Span) -> Value {
    let source_file = source_map.get(file);
    let (start_line, start_col) = source_file.line_col(span.start);
    let (end_line, end_col) = source_file.line_col(span.end);
    json!({
        "start": {
            "line": start_line.saturating_sub(1),
            "character": start_col.saturating_sub(1),
        },
        "end": {
            "line": end_line.saturating_sub(1),
            "character": end_col.saturating_sub(1),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{diagnostic_to_lsp, document_symbols, lsp_severity, uri_to_file_name};
    use daram_compiler::{
        diagnostics::{Diagnostic, Level},
        source::{SourceMap, Span},
    };

    #[test]
    fn converts_file_uri_to_path() {
        assert_eq!(
            uri_to_file_name("file:///tmp/example.dr"),
            "/tmp/example.dr".to_string()
        );
    }

    #[test]
    fn converts_diagnostics_to_lsp_ranges() {
        let mut source_map = SourceMap::new();
        let file = source_map.add_file(
            "sample.dr".to_string(),
            "fun main() {\n    nope\n}\n".to_string(),
        );
        let diagnostic =
            Diagnostic::error("cannot find value `nope`").with_span(Span::new(file, 17, 21));
        let value = diagnostic_to_lsp(&source_map, &diagnostic, "sample.dr")
            .expect("expected lsp diagnostic");
        assert_eq!(value["severity"], lsp_severity(Level::Error));
        assert_eq!(value["range"]["start"]["line"], 1);
        assert_eq!(value["range"]["start"]["character"], 4);
    }

    #[test]
    fn builds_document_symbols_for_top_level_items() {
        let symbols = document_symbols(
            r#"
            export struct Point {
                export x: i32,
                export y: i32,
            }

            export ability Display {
                fun fmt(self): i32;
            }

            fun main(): i32 { 0 }
            "#,
            "sample.dr",
        );

        let names = symbols
            .iter()
            .filter_map(|value| value.get("name").and_then(|name| name.as_str()))
            .collect::<Vec<_>>();
        assert!(names.contains(&"Point"));
        assert!(names.contains(&"Display"));
        assert!(names.contains(&"main"));
    }
}
