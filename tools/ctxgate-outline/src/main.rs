//! ctxgate-outline — emit a symbol outline (kind, name, line range) for a source file.
//!
//! This is the ONLY non-Almide component of ctxgate, and deliberately dumb: it parses
//! with tree-sitter (16 grammars) and prints JSON. Also usable on its own, e.g. as
//! `HEW_OUTLINE_BIN` for https://github.com/O6lvl4/hew. All decisions (what to show, how much) live in the
//! Almide side. The contract is the JSON shape below; a future Almide tree-sitter can
//! replace this binary without touching callers.
//!
//!   ctxgate-outline <file>            → {"path","lang","total_lines","symbols":[{kind,name,start,end}]}
//!   ctxgate-outline --lang rust -     → read source from stdin
//!   ctxgate-outline --fetch lang…     → download grammars ahead of time
//!   ctxgate-outline --languages       → what is supported
//!
//! 16 languages have hand-written symbol rules and statically linked grammars. Every other
//! language known to tree-sitter-language-pack (371) is parsed with a grammar downloaded on
//! first use (cached under the platform cache dir; TREE_SITTER_LANGUAGE_PACK_CACHE_DIR
//! overrides) and outlined with a generic rule: a named node whose kind says definition /
//! declaration / function / class … and which has a name.
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
    C,
    Cpp,
    Java,
    Ruby,
    CSharp,
    Php,
    Bash,
    Lua,
    Kotlin,
    Swift,
    Scala,
    /// Any other language: grammar from tree-sitter-language-pack, generic symbol rules.
    Pack(String),
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
        "c" | "h" => Some(Lang::C),
        "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "ipp" => Some(Lang::Cpp),
        "java" => Some(Lang::Java),
        "rb" | "rake" | "gemspec" | "ruby" => Some(Lang::Ruby),
        "cs" | "csharp" => Some(Lang::CSharp),
        "php" | "phtml" => Some(Lang::Php),
        "sh" | "bash" | "zsh" => Some(Lang::Bash),
        "lua" => Some(Lang::Lua),
        "kt" | "kts" | "kotlin" => Some(Lang::Kotlin),
        "swift" => Some(Lang::Swift),
        "scala" | "sc" => Some(Lang::Scala),
        _ => {
            if let Some(f) = forced {
                return Some(Lang::Pack(f.to_string()));
            }
            tree_sitter_language_pack::detect_language(path).map(|n| Lang::Pack(n.to_string()))
        }
    }
}

fn grammar(l: &Lang) -> Result<(Language, String), String> {
    if let Lang::Pack(name) = l {
        return match tree_sitter_language_pack::get_language(name) {
            Ok(lang) => Ok((lang, name.clone())),
            Err(e) => Err(format!("{name}: {e}")),
        };
    }
    let (lang, name): (Language, &'static str) = match l {
        Lang::Pack(_) => unreachable!(),
        Lang::Rust => (tree_sitter_rust::LANGUAGE.into(), "rust"),
        Lang::Go => (tree_sitter_go::LANGUAGE.into(), "go"),
        Lang::Ts => (tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(), "typescript"),
        Lang::Tsx => (tree_sitter_typescript::LANGUAGE_TSX.into(), "tsx"),
        Lang::Py => (tree_sitter_python::LANGUAGE.into(), "python"),
        Lang::C => (tree_sitter_c::LANGUAGE.into(), "c"),
        Lang::Cpp => (tree_sitter_cpp::LANGUAGE.into(), "cpp"),
        Lang::Java => (tree_sitter_java::LANGUAGE.into(), "java"),
        Lang::Ruby => (tree_sitter_ruby::LANGUAGE.into(), "ruby"),
        Lang::CSharp => (tree_sitter_c_sharp::LANGUAGE.into(), "csharp"),
        Lang::Php => (tree_sitter_php::LANGUAGE_PHP.into(), "php"),
        Lang::Bash => (tree_sitter_bash::LANGUAGE.into(), "bash"),
        Lang::Lua => (tree_sitter_lua::LANGUAGE.into(), "lua"),
        Lang::Kotlin => (tree_sitter_kotlin_ng::LANGUAGE.into(), "kotlin"),
        Lang::Swift => (tree_sitter_swift::LANGUAGE.into(), "swift"),
        Lang::Scala => (tree_sitter_scala::LANGUAGE.into(), "scala"),
    };
    Ok((lang, name.to_string()))
}

fn text<'a>(n: Node, src: &'a [u8]) -> &'a str {
    n.utf8_text(src).unwrap_or("")
}

fn field(n: Node, name: &str, src: &[u8]) -> String {
    n.child_by_field_name(name).map(|c| text(c, src).to_string()).unwrap_or_default()
}

/// The identifier at the bottom of a C / C++ declarator chain (`*foo(int)` → `foo`, `Foo::bar` kept).
fn declarator_name(n: Node, src: &[u8]) -> String {
    let mut cur = n;
    loop {
        match cur.kind() {
            "identifier" | "field_identifier" | "qualified_identifier" | "destructor_name" | "operator_name"
            | "type_identifier" => return text(cur, src).to_string(),
            _ => match cur.child_by_field_name("declarator") {
                Some(d) => cur = d,
                None => {
                    let mut c = cur.walk();
                    let inner = cur.children(&mut c).find(|ch| ch.kind().ends_with("declarator") || ch.kind() == "identifier");
                    match inner {
                        Some(i) => cur = i,
                        None => return text(cur, src).to_string(),
                    }
                }
            },
        }
    }
}

/// First child (named or keyword) of one of the given kinds, as text (for grammars without a `name` field).
fn child_text(n: Node, kinds: &[&str], src: &[u8]) -> String {
    let mut c = n.walk();
    n.children(&mut c)
        .find(|ch| kinds.contains(&ch.kind()))
        .map(|ch| text(ch, src).to_string())
        .unwrap_or_default()
}

const GENERIC_KINDS: &[&str] = &[
    "function", "method", "class", "struct", "enum", "interface", "trait", "impl", "module", "namespace",
    "object", "protocol", "definition", "declaration", "typedef", "type_alias", "macro", "record", "union",
    "constructor", "procedure", "subroutine", "rule", "task", "entity", "package", "fnproto", "vardecl",
    "data_type", "newtype", "signature",
];
const GENERIC_NOT: &[&str] = &[
    "call", "expression", "argument", "parameter", "reference", "access", "invocation", "import", "use_",
    "type_arguments", "annotation", "attribute", "literal", "pattern", "field_", "variable_declaration",
    "assignment", "local_", "constructors", "_body", "_list", "block",
];
const CONTAINER_KINDS: &[&str] = &["class", "struct", "impl", "trait", "interface", "module", "namespace", "object", "protocol", "record", "enum", "entity", "package", "vardecl"];
const NAME_FIELDS: &[&str] = &["name", "function", "variable_type_function", "declarator", "pattern", "identifier"];

fn generic_name(n: Node, src: &[u8]) -> String {
    for f in NAME_FIELDS {
        if let Some(c) = n.child_by_field_name(f) {
            let t = match c.kind() {
                k if k.ends_with("declarator") => declarator_name(c, src),
                _ => text(c, src).to_string(),
            };
            if !t.is_empty() {
                return t;
            }
        }
    }
    let mut c = n.walk();
    n.named_children(&mut c)
        .find(|ch| {
            let k = ch.kind().to_ascii_lowercase();
            !k.starts_with("type_") && (k.ends_with("identifier") || k == "name" || k.ends_with("_name") || k == "variable" || k == "alias")
        })
        .map(|ch| text(ch, src).to_string())
        .unwrap_or_default()
}

/// Language-agnostic rule for grammars we have no table for, plus a few one-line specials
/// for grammars that model declarations as calls or bindings.
fn classify_generic(lang: &str, n: Node, src: &[u8]) -> Option<(&'static str, String, bool)> {
    let k = n.kind().to_ascii_lowercase();
    // Elixir: `def name(...)`, `defmodule Name do` are `call` nodes whose target is the macro.
    if lang == "elixir" && k == "call" {
        let target = field(n, "target", src);
        let label: &'static str = match target.as_str() {
            "defmodule" => "module",
            "def" | "defp" | "defmacro" | "defmacrop" | "defguard" | "defguardp" => "def",
            "defprotocol" => "protocol",
            "defimpl" => "impl",
            "defstruct" => "struct",
            _ => return None,
        };
        let mut c = n.walk();
        let args = n.named_children(&mut c).find(|ch| ch.kind() == "arguments")?;
        let first = args.named_child(0)?;
        let name = if label == "struct" { "struct".to_string() } else { text(first, src).split('(').next().unwrap_or("").trim().to_string() };
        if name.is_empty() {
            return None;
        }
        return Some((label, name, matches!(label, "module" | "protocol" | "impl")));
    }
    // OCaml / Haskell style: the definition node wraps a binding that carries the name.
    if k == "value_definition" || k == "type_definition" || k == "module_definition" {
        let mut c = n.walk();
        if let Some(b) = n.named_children(&mut c).find(|ch| ch.kind().ends_with("_binding")) {
            let name = generic_name(b, src);
            if !name.is_empty() {
                let label: &'static str = match k.as_str() { "value_definition" => "let", "type_definition" => "type", _ => "module" };
                return Some((label, name, label == "module"));
            }
        }
    }
    if k.ends_with("_binding") || !GENERIC_KINDS.iter().any(|g| k.contains(g)) || GENERIC_NOT.iter().any(|g| k.contains(g)) {
        return None;
    }
    let name = generic_name(n, src);
    if name.is_empty() || name.len() > 120 || name.contains('\n') || name.contains(' ') {
        return None;
    }
    let label: &'static str = match GENERIC_KINDS.iter().find(|g| k.contains(*g)).copied().unwrap_or("symbol") {
        "fnproto" | "signature" => "function",
        "vardecl" => {
            let t = text(n, src);
            if t.contains("struct {") || t.contains("struct(") { "struct" } else if t.contains("enum {") || t.contains("enum(") { "enum" } else if t.contains("union {") || t.contains("union(") { "union" } else { "const" }
        }
        "data_type" | "newtype" => "type",
        other => other,
    };
    let container = CONTAINER_KINDS.iter().any(|g| k.contains(g));
    Some((label, name, container))
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
        Lang::C | Lang::Cpp => match k {
            "function_definition" => n
                .child_by_field_name("declarator")
                .map(|d| ("fn", declarator_name(d, src), false)),
            "struct_specifier" | "class_specifier" | "union_specifier" | "enum_specifier" => {
                // only definitions (with a body), not forward declarations / uses
                if n.child_by_field_name("body").is_none() {
                    return None;
                }
                let kind = match k {
                    "struct_specifier" => "struct",
                    "class_specifier" => "class",
                    "union_specifier" => "union",
                    _ => "enum",
                };
                Some((kind, field(n, "name", src), matches!(l, Lang::Cpp)))
            }
            "namespace_definition" => Some(("namespace", field(n, "name", src), true)),
            "type_definition" => n.child_by_field_name("declarator").map(|d| ("typedef", declarator_name(d, src), false)),
            "preproc_function_def" => Some(("macro", field(n, "name", src), false)),
            "preproc_def" => Some(("define", field(n, "name", src), false)),
            _ => None,
        },
        Lang::Java => match k {
            "class_declaration" => Some(("class", field(n, "name", src), true)),
            "interface_declaration" => Some(("interface", field(n, "name", src), true)),
            "enum_declaration" => Some(("enum", field(n, "name", src), true)),
            "record_declaration" => Some(("record", field(n, "name", src), true)),
            "annotation_type_declaration" => Some(("annotation", field(n, "name", src), false)),
            "method_declaration" => Some(("method", field(n, "name", src), false)),
            "constructor_declaration" => Some(("constructor", field(n, "name", src), false)),
            _ => None,
        },
        Lang::Ruby => match k {
            "class" => Some(("class", field(n, "name", src), true)),
            "module" => Some(("module", field(n, "name", src), true)),
            "method" => Some(("def", field(n, "name", src), false)),
            "singleton_method" => Some(("def", format!("self.{}", field(n, "name", src)), false)),
            _ => None,
        },
        Lang::CSharp => match k {
            "namespace_declaration" | "file_scoped_namespace_declaration" => Some(("namespace", field(n, "name", src), true)),
            "class_declaration" => Some(("class", field(n, "name", src), true)),
            "struct_declaration" => Some(("struct", field(n, "name", src), true)),
            "interface_declaration" => Some(("interface", field(n, "name", src), true)),
            "enum_declaration" => Some(("enum", field(n, "name", src), false)),
            "record_declaration" | "record_struct_declaration" => Some(("record", field(n, "name", src), true)),
            "delegate_declaration" => Some(("delegate", field(n, "name", src), false)),
            "method_declaration" => Some(("method", field(n, "name", src), false)),
            "constructor_declaration" => Some(("constructor", field(n, "name", src), false)),
            "property_declaration" => Some(("property", field(n, "name", src), false)),
            _ => None,
        },
        Lang::Php => match k {
            "function_definition" => Some(("function", field(n, "name", src), false)),
            "class_declaration" => Some(("class", field(n, "name", src), true)),
            "interface_declaration" => Some(("interface", field(n, "name", src), true)),
            "trait_declaration" => Some(("trait", field(n, "name", src), true)),
            "enum_declaration" => Some(("enum", field(n, "name", src), true)),
            "method_declaration" => Some(("method", field(n, "name", src), false)),
            "namespace_definition" => Some(("namespace", field(n, "name", src), true)),
            _ => None,
        },
        Lang::Bash => match k {
            "function_definition" => Some(("function", field(n, "name", src), false)),
            _ => None,
        },
        Lang::Lua => match k {
            "function_declaration" => Some(("function", field(n, "name", src), false)),
            _ => None,
        },
        Lang::Kotlin => match k {
            "class_declaration" => Some(("class", child_text(n, &["type_identifier", "identifier", "simple_identifier"], src), true)),
            "object_declaration" => Some(("object", child_text(n, &["type_identifier", "identifier", "simple_identifier"], src), true)),
            "function_declaration" => Some(("fun", child_text(n, &["simple_identifier", "identifier"], src), false)),
            "property_declaration" => None,
            _ => None,
        },
        Lang::Swift => match k {
            "class_declaration" => {
                let name = field(n, "name", src);
                let kw = child_text(n, &["class", "struct", "enum", "extension", "actor"], src);
                let kind: &'static str = match kw.as_str() {
                    "struct" => "struct",
                    "enum" => "enum",
                    "extension" => "extension",
                    "actor" => "actor",
                    _ => "class",
                };
                Some((kind, name, true))
            }
            "protocol_declaration" => Some(("protocol", field(n, "name", src), true)),
            "function_declaration" => Some(("func", field(n, "name", src), false)),
            "init_declaration" => Some(("init", "init".to_string(), false)),
            _ => None,
        },
        Lang::Scala => match k {
            "class_definition" => Some(("class", field(n, "name", src), true)),
            "object_definition" => Some(("object", field(n, "name", src), true)),
            "trait_definition" => Some(("trait", field(n, "name", src), true)),
            "function_definition" => Some(("def", field(n, "name", src), false)),
            "val_definition" | "var_definition" => None,
            _ => None,
        },
        Lang::Pack(name) => classify_generic(name, n, src),
    }
}

fn walk(l: &Lang, n: Node, src: &[u8], prefix: &str, out: &mut Vec<Sym>) {
    let mut cursor = n.walk();
    for child in n.children(&mut cursor) {
        if !child.is_named() {
            continue; // keyword tokens share kind names with the nodes they introduce (`class`, `module`)
        }
        match classify(l, child, src) {
            Some((kind, name, container)) => {
                let full = if prefix.is_empty() { name.clone() } else { format!("{prefix}{}{name}", sep(l)) };
                // A variable_declarator inside a lexical_declaration spans only the declarator;
                // use the enclosing statement's range so the whole `const x = ...` is covered.
                let pk = child.parent().map(|p| p.kind().to_ascii_lowercase()).unwrap_or_default();
                let wrapper = child.kind() == "variable_declarator"
                    || (matches!(l, Lang::Pack(_)) && child.parent().map(|p| p.parent().is_some()).unwrap_or(false)
                        && (pk == "decl" || pk.ends_with("_declaration") || pk.ends_with("_definition") || pk == "declaration"));
                let range_node = if wrapper { child.parent().unwrap_or(child) } else { child };
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

/// Debug aid for writing rules: named nodes with their field names, depth-limited.
fn dump_tree(n: Node, src: &[u8], depth: usize) {
    if depth > 4 {
        return;
    }
    let mut c = n.walk();
    for (idx, ch) in n.children(&mut c).enumerate() {
        if !ch.is_named() {
            continue;
        }
        let fname = n.field_name_for_child(idx as u32).unwrap_or("");
        let t = text(ch, src);
        let short: String = t.lines().next().unwrap_or("").chars().take(50).collect();
        eprintln!("{}{}{}  L{}  {:?}", "  ".repeat(depth), if fname.is_empty() { String::new() } else { format!("{fname}: ") }, ch.kind(), ch.start_position().row + 1, short);
        dump_tree(ch, src, depth + 1);
    }
}

fn sep(l: &Lang) -> &'static str {
    match l {
        Lang::Rust | Lang::Cpp | Lang::Php => "::",
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
        Lang::C | Lang::Cpp => matches!(kind, "compound_statement" | "preproc_include"),
        Lang::Java | Lang::CSharp => matches!(kind, "block" | "import_declaration" | "using_directive"),
        Lang::Ruby => matches!(kind, "do_block" | "block"),
        Lang::Php => matches!(kind, "compound_statement"),
        Lang::Bash => matches!(kind, "compound_statement"),
        Lang::Lua => matches!(kind, "block"),
        Lang::Kotlin | Lang::Swift | Lang::Scala => matches!(kind, "function_body" | "block" | "import_declaration" | "import_list"),
        Lang::Pack(_) => matches!(kind, "block" | "compound_statement" | "statement_block" | "function_body"),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut forced: Option<String> = None;
    let mut path: Option<String> = None;
    let mut dump = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lang" => {
                forced = args.get(i + 1).cloned();
                i += 2;
            }
            "-h" | "--help" => {
                eprintln!("usage: ctxgate-outline [--lang L] <file | ->   |   --fetch L…   |   --languages");
                return ExitCode::SUCCESS;
            }
            "--languages" => {
                let mut names = tree_sitter_language_pack::manifest_languages().unwrap_or_else(|_| tree_sitter_language_pack::available_languages());
                names.sort();
                let cached = tree_sitter_language_pack::downloaded_languages().len();
                println!("16 with dedicated rules: rust go typescript tsx python c cpp java ruby csharp php bash lua kotlin swift scala");
                println!("{} via tree-sitter-language-pack ({} cached, the rest downloaded on first use):", names.len(), cached);
                println!("{}", names.join(" "));
                return ExitCode::SUCCESS;
            }
            "--dump" => {
                dump = true;
                i += 1;
            }
            "--fetch" => {
                let names: Vec<&str> = args[i + 1..].iter().map(|s| s.as_str()).collect();
                return match tree_sitter_language_pack::download(&names) {
                    Ok(n) => {
                        eprintln!("ctxgate-outline: {n} grammar(s) ready");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("ctxgate-outline: fetch failed: {e}");
                        ExitCode::from(1)
                    }
                };
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
        Some(l) => match grammar(&l) {
            Ok((language, name)) => {
                let mut parser = Parser::new();
                if parser.set_language(&language).is_ok() {
                    if let Some(tree) = parser.parse(&src, None) {
                        if dump {
                            dump_tree(tree.root_node(), &src, 0);
                        }
                        walk(&l, tree.root_node(), &src, "", &mut symbols);
                    }
                }
                name
            }
            Err(e) => {
                eprintln!("ctxgate-outline: grammar unavailable ({e})");
                "unknown".to_string()
            }
        },
        None => "unknown".to_string(),
    };

    // Grammars that split one thing over several nodes (a signature, then equations) produce
    // adjacent symbols with the same name: fold them into one range.
    let mut merged: Vec<Sym> = Vec::new();
    for s in symbols {
        match merged.last_mut() {
            Some(prev) if prev.name == s.name && s.start <= prev.end + 1 => prev.end = prev.end.max(s.end),
            _ => merged.push(s),
        }
    }
    let symbols = merged;
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
