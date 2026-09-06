//! ctxgate-outline — emit a symbol outline (kind, name, line range) for a source file.
//!
//! This is the ONLY non-Almide component of ctxgate, and deliberately dumb: it parses
//! with tree-sitter and prints JSON. All decisions (what to show, how much) live in the
//! Almide side. The contract is the JSON shape below; a future Almide tree-sitter can
//! replace this binary without touching callers.
//!
//!   ctxgate-outline <file>            → {"path","lang","total_lines","symbols":[{kind,name,start,end}]}
//!   ctxgate-outline --lang rust -     → read source from stdin
//!
//! Lines are 1-based and inclusive. Exit 0 with an empty `symbols` array for unsupported
//! languages, so callers can fall back without parsing stderr.

use std::io::Read;
use std::process::ExitCode;
use tree_sitter::{Language, Node, Parser};

struct Sym {
    kind: &'static str,
    name: String,
    start: usize,
    end: usize,
}

enum Lang {
    Rust,
    Go,
    Ts,
    Tsx,
    Py,
}

fn lang_of(path: &str, forced: Option<&str>) -> Option<Lang> {
    let ext = forced
        .map(|s| s.to_string())
        .unwrap_or_else(|| path.rsplit('.').next().unwrap_or("").to_ascii_lowercase());
    match ext.as_str() {
        "rs" | "rust" => Some(Lang::Rust),
        "go" => Some(Lang::Go),
        "ts" | "mts" | "cts" | "typescript" | "js" | "mjs" | "cjs" => Some(Lang::Ts),
        "tsx" | "jsx" => Some(Lang::Tsx),
        "py" | "pyi" | "python" => Some(Lang::Py),
        _ => None,
    }
}

fn grammar(l: &Lang) -> (Language, &'static str) {
    match l {
        Lang::Rust => (tree_sitter_rust::LANGUAGE.into(), "rust"),
        Lang::Go => (tree_sitter_go::LANGUAGE.into(), "go"),
        Lang::Ts => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), "typescript"),
        Lang::Tsx => (tree_sitter_typescript::LANGUAGE_TSX.into(), "tsx"),
        Lang::Py => (tree_sitter_python::LANGUAGE.into(), "python"),
    }
}

fn text<'a>(n: Node, src: &'a [u8]) -> &'a str {
    n.utf8_text(src).unwrap_or("")
}

fn field(n: Node, name: &str, src: &[u8]) -> String {
    n.child_by_field_name(name).map(|c| text(c, src).to_string()).unwrap_or_default()
}

/// Classify a node: Some((kind label, name, is_container)) when it is a symbol we report.
fn classify(l: &Lang, n: Node, src: &[u8]) -> Option<(&'static str, String, bool)> {
    let k = n.kind();
    match l {
        Lang::Rust => match k {
            "function_item" | "function_signature_item" => Some(("fn", field(n, "name", src), false)),
            "impl_item" => {
                let ty = field(n, "type", src);
                let tr = field(n, "trait", src);
                let name = if tr.is_empty() { ty } else { format!("{tr} for {ty}") };
                Some(("impl", name, true))
            }
            "struct_item" => Some(("struct", field(n, "name", src), false)),
            "enum_item" => Some(("enum", field(n, "name", src), false)),
            "union_item" => Some(("union", field(n, "name", src), false)),
            "trait_item" => Some(("trait", field(n, "name", src), true)),
            "type_item" => Some(("type", field(n, "name", src), false)),
            "mod_item" => Some(("mod", field(n, "name", src), true)),
            "macro_definition" => Some(("macro", field(n, "name", src), false)),
            "const_item" => Some(("const", field(n, "name", src), false)),
            "static_item" => Some(("static", field(n, "name", src), false)),
            _ => None,
        },
        Lang::Go => match k {
            "function_declaration" => Some(("func", field(n, "name", src), false)),
            "method_declaration" => {
                // receiver: parameter_list → parameter_declaration → type
                let recv = n
                    .child_by_field_name("receiver")
                    .and_then(|r| r.named_child(0))
                    .and_then(|p| p.child_by_field_name("type"))
                    .map(|t| text(t, src).trim_start_matches('*').to_string())
                    .unwrap_or_default();
                Some(("method", format!("{recv}.{}", field(n, "name", src)), false))
            }
            "type_spec" => {
                let kind = n
                    .child_by_field_name("type")
                    .map(|t| match t.kind() {
                        "struct_type" => "struct",
                        "interface_type" => "interface",
                        _ => "type",
                    })
                    .unwrap_or("type");
                Some((kind, field(n, "name", src), false))
            }
            _ => None,
        },
        Lang::Ts | Lang::Tsx => match k {
            "function_declaration" | "generator_function_declaration" => {
                Some(("function", field(n, "name", src), false))
            }
            "class_declaration" | "abstract_class_declaration" => Some(("class", field(n, "name", src), true)),
            "method_definition" => Some(("method", field(n, "name", src), false)),
            "interface_declaration" => Some(("interface", field(n, "name", src), false)),
            "type_alias_declaration" => Some(("type", field(n, "name", src), false)),
            "enum_declaration" => Some(("enum", field(n, "name", src), false)),
            "module" | "internal_module" => Some(("namespace", field(n, "name", src), true)),
            "variable_declarator" => {
                // const foo = (...) => {...}  /  const foo = function () {}
                let is_fn = n
                    .child_by_field_name("value")
                    .map(|v| matches!(v.kind(), "arrow_function" | "function_expression" | "function"))
                    .unwrap_or(false);
                if is_fn {
                    Some(("function", field(n, "name", src), false))
                } else {
                    None
                }
            }
            _ => None,
        },
        Lang::Py => match k {
            "function_definition" => Some(("def", field(n, "name", src), false)),
            "class_definition" => Some(("class", field(n, "name", src), true)),
            _ => None,
        },
    }
}

fn walk(l: &Lang, n: Node, src: &[u8], prefix: &str, out: &mut Vec<Sym>) {
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        match classify(l, child, src) {
            Some((kind, name, container)) => {
                let full = if prefix.is_empty() { name.clone() } else { format!("{prefix}{}{name}", sep(l)) };
                // A variable_declarator inside a lexical_declaration spans only the declarator;
                // use the enclosing statement's range so the whole `const x = ...` is covered.
                let range_node = if child.kind() == "variable_declarator" { child.parent().unwrap_or(child) } else { child };
                out.push(Sym {
                    kind,
                    name: full.clone(),
                    start: range_node.start_position().row + 1,
                    end: range_node.end_position().row + 1,
                });
                if container {
                    walk(l, child, src, &full, out);
                }
            }
            None => {
                // Transparent wrappers (export_statement, decorated_definition, declaration_list,
                // class_body, block of an impl…) — keep looking inside, same prefix.
                if !is_body(l, child.kind()) {
                    walk(l, child, src, prefix, out);
                }
            }
        }
    }
}

fn sep(l: &Lang) -> &'static str {
    match l {
        Lang::Rust => "::",
        _ => ".",
    }
}

/// Node kinds whose interior is executable code — do not descend (skips closures, inner fns).
fn is_body(l: &Lang, kind: &str) -> bool {
    match l {
        Lang::Rust => matches!(kind, "block" | "attribute_item" | "use_declaration"),
        Lang::Go => matches!(kind, "block" | "import_declaration"),
        Lang::Ts | Lang::Tsx => matches!(kind, "statement_block" | "import_statement"),
        Lang::Py => matches!(kind, "import_statement" | "import_from_statement" | "expression_statement"),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut forced: Option<String> = None;
    let mut path: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lang" => {
                forced = args.get(i + 1).cloned();
                i += 2;
            }
            "-h" | "--help" => {
                eprintln!("usage: ctxgate-outline [--lang rust|go|ts|tsx|py] <file | ->");
                return ExitCode::SUCCESS;
            }
            p => {
                path = Some(p.to_string());
                i += 1;
            }
        }
    }
    let Some(path) = path else {
        eprintln!("usage: ctxgate-outline [--lang L] <file | ->");
        return ExitCode::from(2);
    };

    let src = if path == "-" {
        let mut buf = Vec::new();
        if std::io::stdin().read_to_end(&mut buf).is_err() {
            return ExitCode::from(2);
        }
        buf
    } else {
        match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("ctxgate-outline: {path}: {e}");
                return ExitCode::from(2);
            }
        }
    };
    let total_lines = src.iter().filter(|&&b| b == b'\n').count() + usize::from(!src.is_empty() && *src.last().unwrap() != b'\n');

    let mut symbols: Vec<Sym> = Vec::new();
    let lang_name = match lang_of(&path, forced.as_deref()) {
        Some(l) => {
            let (language, name) = grammar(&l);
            let mut parser = Parser::new();
            if parser.set_language(&language).is_ok() {
                if let Some(tree) = parser.parse(&src, None) {
                    walk(&l, tree.root_node(), &src, "", &mut symbols);
                }
            }
            name
        }
        None => "unknown",
    };

    let syms: Vec<serde_json::Value> = symbols
        .iter()
        .map(|s| serde_json::json!({"kind": s.kind, "name": s.name, "start": s.start, "end": s.end}))
        .collect();
    let out = serde_json::json!({
        "path": path,
        "lang": lang_name,
        "total_lines": total_lines,
        "symbols": syms,
    });
    println!("{out}");
    ExitCode::SUCCESS
}
