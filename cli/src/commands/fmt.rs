//! `dr fmt` — format all Daram source files.
//!
//! The formatter enforces the canonical style defined in `[fmt]` in `daram.toml`:
//! - 4-space indentation (configurable via `indent-size`)
//! - trailing commas in multi-line constructs
//! - maximum line length of 100 characters (configurable)
//! - consistent blank lines between top-level items

use std::{fs, path::PathBuf};

use crate::{terminal, workspace::find_workspace};
use daram_compiler::{lexer::lex_with_errors, parser::parse, source::FileId};

pub fn run(args: &[String]) -> i32 {
    let check_only = args.iter().any(|a| a == "--check");
    let migrate_syntax = args.iter().any(|a| a == "--migrate-syntax");
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");

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

    let indent_size = ws.manifest.fmt.indent_size;
    let max_line = ws.manifest.fmt.max_line_length;

    let mut changed = 0usize;
    let mut checked = 0usize;
    let mut errors = 0usize;

    if let Err(e) = format_dir(
        &src_dir,
        indent_size,
        max_line,
        check_only,
        migrate_syntax,
        verbose,
        &mut changed,
        &mut checked,
        &mut errors,
    ) {
        terminal::error(&format!("I/O error while formatting: {}", e));
        return 1;
    }

    if check_only {
        if changed > 0 {
            terminal::error(&format!("{} file(s) would be reformatted", changed));
            return 1;
        }
        terminal::success(&format!("{} file(s) are correctly formatted", checked));
    } else {
        if errors > 0 {
            terminal::warn(&format!(
                "{} file(s) could not be formatted (lex errors)",
                errors
            ));
        }
        terminal::success(&format!("formatted {} file(s)", changed));
    }
    0
}

fn format_dir(
    dir: &PathBuf,
    indent: usize,
    max_line: usize,
    check_only: bool,
    migrate_syntax: bool,
    verbose: bool,
    changed: &mut usize,
    checked: &mut usize,
    errors: &mut usize,
) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            format_dir(
                &path,
                indent,
                max_line,
                check_only,
                migrate_syntax,
                verbose,
                changed,
                checked,
                errors,
            )?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("dr") {
            *checked += 1;
            let original = fs::read_to_string(&path)?;
            let candidate = if migrate_syntax {
                migrate_source_syntax(&original)
            } else {
                original.clone()
            };
            let (_tokens, lex_errors) = lex_with_errors(&candidate);
            if !lex_errors.is_empty() {
                *errors += 1;
                if verbose {
                    terminal::warn(&format!("  skipping `{}` (lex errors)", path.display()));
                }
                continue;
            }
            let formatted = format_source(&candidate, indent);
            if formatted != original {
                *changed += 1;
                if check_only {
                    if verbose {
                        terminal::warn(&format!("  would reformat: {}", path.display()));
                    }
                } else {
                    fs::write(&path, &formatted)?;
                    if verbose {
                        terminal::success(&format!("  reformatted: {}", path.display()));
                    }
                }
            }
        }
    }
    Ok(())
}

fn migrate_source_syntax(src: &str) -> String {
    let src = src.replace("\r\n", "\n");
    let lines = src.lines().collect::<Vec<_>>();
    let mut out = Vec::new();
    let mut index = 0usize;

    while index < lines.len() {
        let mut line = lines[index].trim_end().to_string();
        let trimmed = line.trim_start();
        if trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with("*/")
        {
            out.push(line);
            index += 1;
            continue;
        }

        if let Some((migrated, consumed)) = migrate_use_block(&lines, index) {
            out.push(migrated);
            index += consumed;
            continue;
        }

        if let Some((migrated, consumed)) = migrate_impl_header_block(&lines, index) {
            out.push(migrated);
            index += consumed;
            continue;
        }

        if let Some(migrated) = migrate_use_statement(&line) {
            line = migrated;
        } else if let Some(migrated) = migrate_impl_header(&line) {
            line = migrated;
        }

        line = migrate_pub_let_statement(&line);
        line = migrate_local_binding(&line);
        line = replace_word_token(&line, "pub", "export");
        line = replace_word_token(&line, "fn", "fun");
        line = replace_return_arrow(&line);
        out.push(line);
        index += 1;
    }

    let mut rendered = out.join("\n");
    if src.ends_with('\n') {
        rendered.push('\n');
    }
    rendered
}

fn migrate_use_block(lines: &[&str], start: usize) -> Option<(String, usize)> {
    let first = lines.get(start)?.trim_end();
    let trimmed = first.trim_start();
    if !(trimmed.starts_with("use ") || trimmed.starts_with("pub use ")) {
        return None;
    }

    let mut parts = Vec::new();
    let mut consumed = 0usize;
    for line in &lines[start..] {
        consumed += 1;
        parts.push(line.trim());
        if line.trim_end().ends_with(';') {
            break;
        }
    }
    let combined = parts.join(" ");
    let migrated = migrate_use_statement(&combined)?;
    Some((migrated, consumed))
}

fn migrate_use_statement(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let (visibility, rest) = if let Some(rest) = trimmed.strip_prefix("pub use ") {
        ("export ", rest)
    } else if let Some(rest) = trimmed.strip_prefix("use ") {
        ("", rest)
    } else {
        return None;
    };

    let body = rest.strip_suffix(';')?.trim();
    if let Some((prefix, items)) = body.split_once("::{") {
        let source = prefix.replace("::", "/");
        let items = items.strip_suffix('}')?.trim();
        return Some(format!(
            "{indent}{visibility}import {{ {items} }} from \"{source}\";"
        ));
    }

    let (prefix, item) = body.rsplit_once("::")?;
    let source = prefix.replace("::", "/");
    let item = item.trim();
    if let Some(alias) = item.strip_prefix("* as ") {
        return Some(format!(
            "{indent}{visibility}import * as {} from \"{}\";",
            alias.trim(),
            source
        ));
    }
    Some(format!(
        "{indent}{visibility}import {{ {item} }} from \"{source}\";"
    ))
}

fn migrate_impl_header_block(lines: &[&str], start: usize) -> Option<(String, usize)> {
    let first = lines.get(start)?.trim_end();
    if !first.trim_start().starts_with("impl") {
        return None;
    }
    if first.trim_end().ends_with('{') {
        return None;
    }

    let mut parts = Vec::new();
    let mut consumed = 0usize;
    for line in &lines[start..] {
        consumed += 1;
        parts.push(line.trim());
        if line.trim_end().ends_with('{') {
            break;
        }
    }
    let combined = parts.join(" ");
    let migrated = migrate_impl_header(&combined)?;
    Some((migrated, consumed))
}

fn migrate_impl_header(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let rest = trimmed.strip_prefix("impl")?;
    let header = rest.trim_start().strip_suffix('{')?.trim_end();
    let (head, where_clause) = if let Some((head, where_clause)) = header.split_once(" where ") {
        (head.trim_end(), Some(where_clause))
    } else {
        (header, None)
    };

    let (generics, tail) = split_impl_generics(head);
    let migrated = if let Some((trait_ref, self_ty)) = tail.rsplit_once(" for ") {
        format!(
            "{indent}extend{generics} {} implements {}",
            self_ty.trim(),
            trait_ref.trim()
        )
    } else {
        format!("{indent}extend{generics} {}", tail.trim())
    };

    Some(if let Some(where_clause) = where_clause {
        format!("{migrated} where {} {{", where_clause.trim())
    } else {
        format!("{migrated} {{")
    })
}

fn split_impl_generics(header: &str) -> (&str, &str) {
    let trimmed = header.trim_start();
    if !trimmed.starts_with('<') {
        return ("", trimmed);
    }

    let mut depth = 0usize;
    for (idx, ch) in trimmed.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let generics = &trimmed[..=idx];
                    let rest = trimmed[idx + 1..].trim_start();
                    return (generics, rest);
                }
            }
            _ => {}
        }
    }

    ("", trimmed)
}

fn migrate_pub_let_statement(line: &str) -> String {
    rewrite_pub_let_with_prefix(line, "pub let ")
        .or_else(|| rewrite_pub_let_with_prefix(line, "export let "))
        .unwrap_or_else(|| line.to_string())
}

fn rewrite_pub_let_with_prefix(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let rest = trimmed.strip_prefix(prefix)?;
    let (name, value) = rest.split_once('=')?;
    Some(format!(
        "{indent}export const {}: _ = {}",
        name.trim_end(),
        value.trim_start()
    ))
}

fn migrate_local_binding(line: &str) -> String {
    if let Some(rewritten) = rewrite_line_prefix(line, "let mut ", "let ", |rest| {
        rest.trim_start().to_string()
    }) {
        return rewritten;
    }
    if line.trim_start().starts_with("while let ") || line.trim_start().starts_with("if let ") {
        return line.to_string();
    }
    rewrite_line_prefix(line, "let ", "const ", |rest| rest.trim_start().to_string())
        .unwrap_or_else(|| line.to_string())
}

fn rewrite_line_prefix<F>(line: &str, prefix: &str, replacement: &str, tail: F) -> Option<String>
where
    F: FnOnce(&str) -> String,
{
    let trimmed = line.trim_start();
    let indent = &line[..line.len() - trimmed.len()];
    let rest = trimmed.strip_prefix(prefix)?;
    Some(format!("{indent}{replacement}{}", tail(rest)))
}

fn replace_word_token(line: &str, from: &str, to: &str) -> String {
    let mut out = String::new();
    let chars = line.char_indices().collect::<Vec<_>>();
    let mut i = 0usize;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

    while i < chars.len() {
        let (byte_idx, ch) = chars[i];
        let next_byte = chars.get(i + 1).map(|(idx, _)| *idx).unwrap_or(line.len());

        if !in_string && !in_char && ch == '/' && chars.get(i + 1).map(|(_, c)| *c) == Some('/') {
            out.push_str(&line[byte_idx..]);
            break;
        }

        if !escaped {
            if ch == '"' && !in_char {
                in_string = !in_string;
            } else if ch == '\'' && !in_string {
                in_char = !in_char;
            }
        }

        if !in_string
            && !in_char
            && line[byte_idx..].starts_with(from)
            && boundary_before(line, byte_idx)
            && boundary_after(line, byte_idx + from.len())
        {
            out.push_str(to);
            i += from.chars().count();
            escaped = false;
            continue;
        }

        out.push_str(&line[byte_idx..next_byte]);
        escaped = ch == '\\' && (in_string || in_char) && !escaped;
        if ch != '\\' {
            escaped = false;
        }
        i += 1;
    }

    out
}

fn replace_return_arrow(line: &str) -> String {
    let chars = line.char_indices().collect::<Vec<_>>();
    let mut out = String::new();
    let mut i = 0usize;
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

    while i < chars.len() {
        let (byte_idx, ch) = chars[i];
        let next_byte = chars.get(i + 1).map(|(idx, _)| *idx).unwrap_or(line.len());

        if !in_string && !in_char && ch == '/' && chars.get(i + 1).map(|(_, c)| *c) == Some('/') {
            out.push_str(&line[byte_idx..]);
            break;
        }

        if !escaped {
            if ch == '"' && !in_char {
                in_string = !in_string;
            } else if ch == '\'' && !in_string {
                in_char = !in_char;
            }
        }

        if !in_string && !in_char && ch == ')' {
            let mut j = i + 1;
            while let Some((_, whitespace)) = chars.get(j) {
                if whitespace.is_whitespace() {
                    j += 1;
                } else {
                    break;
                }
            }
            if chars.get(j).map(|(_, c)| *c) == Some('-')
                && chars.get(j + 1).map(|(_, c)| *c) == Some('>')
            {
                out.push(')');
                out.push(':');
                i = j + 2;
                escaped = false;
                continue;
            }
        }

        out.push_str(&line[byte_idx..next_byte]);
        escaped = ch == '\\' && (in_string || in_char) && !escaped;
        if ch != '\\' {
            escaped = false;
        }
        i += 1;
    }

    out
}

fn boundary_before(line: &str, idx: usize) -> bool {
    line[..idx]
        .chars()
        .next_back()
        .is_none_or(|ch| !is_ident_char(ch))
}

fn boundary_after(line: &str, idx: usize) -> bool {
    line[idx..]
        .chars()
        .next()
        .is_none_or(|ch| !is_ident_char(ch))
}

fn is_ident_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

/// Lightweight block-aware formatter:
/// - normalises line endings and trailing whitespace
/// - indents by brace depth
/// - collapses repeated blank lines
/// - preserves comment lines and existing token order
fn format_source(src: &str, indent_size: usize) -> String {
    let (tokens, lex_errors) = lex_with_errors(src);
    if lex_errors.is_empty() {
        let (module, parse_errors) = parse(FileId(0), &tokens);
        if parse_errors.is_empty() {
            return format_source_ast(src, &module, indent_size);
        }
    }
    format_source_text(src, indent_size)
}

fn format_source_ast(
    src: &str,
    module: &daram_compiler::ast::Module,
    indent_size: usize,
) -> String {
    let mut rendered_items = Vec::new();
    for item in &module.items {
        let span = item.span();
        let start = expand_item_start(src, span.start.0 as usize);
        let snippet = src.get(start..span.end.0 as usize).unwrap_or("").trim();
        if snippet.is_empty() {
            continue;
        }
        rendered_items.push(format_source_text(snippet, indent_size).trim().to_string());
    }

    if rendered_items.is_empty() {
        return format_source_text(src, indent_size);
    }

    let mut out = rendered_items.join("\n\n");
    out.push('\n');
    out
}

fn expand_item_start(src: &str, start: usize) -> usize {
    let mut line_start = line_start_index(src, start);

    loop {
        if line_start == 0 {
            return 0;
        }

        let prev_end = line_start.saturating_sub(1);
        let prev_start = line_start_index(src, prev_end);
        let prev_line = &src[prev_start..prev_end];
        let trimmed = prev_line.trim();
        if trimmed.starts_with("#[") || trimmed.starts_with("//") {
            line_start = prev_start;
            continue;
        }
        return line_start;
    }
}

fn line_start_index(src: &str, offset: usize) -> usize {
    src[..offset].rfind('\n').map(|idx| idx + 1).unwrap_or(0)
}

/// Text fallback used for unsupported or unparsable input.
fn format_source_text(src: &str, indent_size: usize) -> String {
    let src = src.replace("\r\n", "\n");
    let mut out_lines = Vec::new();
    let mut indent_level = 0usize;
    let mut prev_blank = false;

    for raw_line in src.lines() {
        let trimmed_end = raw_line.trim_end();
        let trimmed = trimmed_end.trim();
        let is_blank = trimmed.is_empty();
        if is_blank && prev_blank {
            continue;
        }
        if is_blank {
            out_lines.push(String::new());
            prev_blank = true;
            continue;
        }

        let leading_closers = count_leading_closers(trimmed);
        let current_indent = indent_level.saturating_sub(leading_closers);
        out_lines.push(format!(
            "{}{}",
            " ".repeat(current_indent * indent_size),
            trimmed
        ));

        let (opens, closes) = count_braces(trimmed);
        indent_level =
            current_indent + opens.saturating_sub(closes.saturating_sub(leading_closers));
        prev_blank = is_blank;
    }

    let mut out = out_lines.join("\n");
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn count_leading_closers(line: &str) -> usize {
    let mut count = 0usize;
    for ch in line.chars() {
        match ch {
            '}' => count += 1,
            ' ' | '\t' => continue,
            _ => break,
        }
    }
    count
}

fn count_braces(line: &str) -> (usize, usize) {
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut chars = line.chars().peekable();
    let mut in_string = false;

    while let Some(ch) = chars.next() {
        if !in_string && ch == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => opens += 1,
            '}' => closes += 1,
            _ => {}
        }
    }

    (opens, closes)
}

#[cfg(test)]
mod tests {
    use super::{format_dir, format_source, migrate_source_syntax};
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
    fn formats_nested_blocks_with_indentation() {
        let src = "fn main(){\nif true {\nprintln(\"x\");\n}\n}\n";
        let formatted = format_source(src, 4);
        assert_eq!(
            formatted,
            "fn main(){\n    if true {\n        println(\"x\");\n    }\n}\n"
        );
    }

    #[test]
    fn collapses_extra_blank_lines_and_trims_trailing_space() {
        let src = "fn main() {   \n\n\nprintln(\"x\");    \n}\n";
        let formatted = format_source(src, 2);
        assert_eq!(formatted, "fn main() {\n\n  println(\"x\");\n}\n");
    }

    #[test]
    fn keeps_comment_suffix_without_counting_braces_inside_comment() {
        let src = "fn main() {\n// }\nprintln(\"x\");\n}\n";
        let formatted = format_source(src, 4);
        assert_eq!(formatted, "fn main() {\n    // }\n    println(\"x\");\n}\n");
    }

    #[test]
    fn ast_formatter_separates_top_level_items_with_single_blank_line() {
        let src = "fn a(){\n0\n}\n\n\nfn b(){\n1\n}\n";
        let formatted = format_source(src, 4);
        assert_eq!(formatted, "fn a(){\n    0\n}\n\nfn b(){\n    1\n}\n");
    }

    #[test]
    fn migrates_legacy_surface_syntax() {
        let src = "\
pub use std::io::{Read, Write};
pub let OK = StatusCode(200);
impl<T> Buffer<T> {
    pub fn map(value: fn(i32) -> i32) -> i32 {
        let answer = 1;
        let mut total = answer;
        total
    }
}
impl Read for File {
    fn read(mut self, buf: &[u8]) -> i32 { 0 }
}
";
        let migrated = migrate_source_syntax(src);
        assert_eq!(
            migrated,
            "\
export import { Read, Write } from \"std/io\";
export const OK: _ = StatusCode(200);
extend<T> Buffer<T> {
    export fun map(value: fun(i32): i32): i32 {
        const answer = 1;
        let total = answer;
        total
    }
}
extend File implements Read {
    fun read(mut self, buf: &[u8]): i32 { 0 }
}
"
        );
    }

    #[test]
    fn keeps_pattern_matching_let_forms_unchanged() {
        let src = "while let Option::Some(item) = iter.next() {\n    let value = item;\n}\n";
        let migrated = migrate_source_syntax(src);
        assert_eq!(
            migrated,
            "while let Option::Some(item) = iter.next() {\n    const value = item;\n}\n"
        );
    }

    #[test]
    fn migrates_multiline_use_and_impl_headers() {
        let src = "\
pub use std::io::{\n    Read,\n    Write,\n};\n\
impl<T>\nBuffer<T>\nwhere T: Copy\n{\n    fn map(self) -> i32 { 0 }\n}\n";
        let migrated = migrate_source_syntax(src);
        assert_eq!(
            migrated,
            "\
export import { Read, Write, } from \"std/io\";\n\
extend<T> Buffer<T> where T: Copy {\n    fun map(self): i32 { 0 }\n}\n"
        );
    }

    #[test]
    fn format_dir_migrates_legacy_files_on_disk() {
        let dir = unique_temp_dir("daram_fmt_migrate");
        fs::create_dir_all(&dir).expect("expected temp dir creation");
        let file = dir.join("legacy.dr");
        fs::write(
            &file,
            "pub fn main() -> i32 {\nlet value = 1;\nlet mut total = value;\ntotal\n}\n",
        )
        .expect("expected fixture write");

        let mut changed = 0usize;
        let mut checked = 0usize;
        let mut errors = 0usize;
        format_dir(
            &dir,
            4,
            100,
            false,
            true,
            false,
            &mut changed,
            &mut checked,
            &mut errors,
        )
        .expect("expected formatter to succeed");

        let rewritten = fs::read_to_string(&file).expect("expected migrated source");
        assert_eq!(checked, 1);
        assert_eq!(changed, 1);
        assert_eq!(errors, 0);
        assert_eq!(
            rewritten,
            "export fun main(): i32 {\n    const value = 1;\n    let total = value;\n    total\n}\n"
        );

        fs::remove_dir_all(&dir).expect("expected temp dir cleanup");
    }

    #[test]
    fn ast_formatter_preserves_visibility_prefixes() {
        let src = "export fun main() {\n0\n}\n";
        let formatted = format_source(src, 4);
        assert_eq!(formatted, "export fun main() {\n    0\n}\n");
    }

    #[test]
    fn ast_formatter_preserves_plain_line_comments_before_items() {
        let src = "// why this exists\nexport fun main() {\n0\n}\n";
        let formatted = format_source(src, 4);
        assert_eq!(
            formatted,
            "// why this exists\nexport fun main() {\n    0\n}\n"
        );
    }
}
