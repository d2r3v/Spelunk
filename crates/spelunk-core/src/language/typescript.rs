//! TypeScript / TSX language config.
//!
//! The chunk query below defines what counts as a "definition" in TypeScript.
//! Each pattern captures the whole definition node as `@definition` and its
//! identifier as `@name`. The chunker (see `chunk.rs`) handles export
//! wrappers, nesting, and size limits generically.

use crate::chunk::ChunkKind;
use crate::language::{Language, LanguageConfig};

/// Shared by the TS and TSX grammars — TSX is a superset with the same node
/// kinds for everything we capture here.
const CHUNK_QUERY: &str = r#"
(function_declaration name: (_) @name) @definition
(generator_function_declaration name: (_) @name) @definition
(class_declaration name: (_) @name) @definition
(abstract_class_declaration name: (_) @name) @definition
(interface_declaration name: (_) @name) @definition
(enum_declaration name: (_) @name) @definition
(type_alias_declaration name: (_) @name) @definition
(method_definition name: (_) @name) @definition

; `const handler = () => { ... }` and `let f = function () { ... }`
(lexical_declaration
  (variable_declarator
    name: (_) @name
    value: [(arrow_function) (function_expression)])) @definition
(variable_declaration
  (variable_declarator
    name: (_) @name
    value: [(arrow_function) (function_expression)])) @definition

; class property methods: `class C { handle = () => { ... } }`
(public_field_definition
  name: (_) @name
  value: [(arrow_function) (function_expression)]) @definition
"#;

fn kind_for_node(node_kind: &str) -> ChunkKind {
    match node_kind {
        "class_declaration" | "abstract_class_declaration" => ChunkKind::Class,
        "interface_declaration" => ChunkKind::Interface,
        "enum_declaration" => ChunkKind::Enum,
        "type_alias_declaration" => ChunkKind::TypeAlias,
        "method_definition" | "public_field_definition" => ChunkKind::Method,
        // function_declaration, generator_function_declaration,
        // lexical_declaration, variable_declaration
        _ => ChunkKind::Function,
    }
}

pub static TYPESCRIPT: LanguageConfig = LanguageConfig::new(
    Language::TypeScript,
    &["ts", "mts", "cts"],
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    CHUNK_QUERY,
    kind_for_node,
);

pub static TSX: LanguageConfig = LanguageConfig::new(
    Language::Tsx,
    &["tsx"],
    tree_sitter_typescript::LANGUAGE_TSX,
    CHUNK_QUERY,
    kind_for_node,
);
