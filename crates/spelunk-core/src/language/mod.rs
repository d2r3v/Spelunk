//! The language layer. Adding a language to spelunk means:
//!
//! 1. Add the grammar crate to `Cargo.toml`.
//! 2. Add one module here that defines a `static LanguageConfig` (grammar,
//!    file extensions, and a tree-sitter chunk query).
//! 3. List it in [`REGISTRY`] and add a variant to [`Language`].
//!
//! Everything else (walking, chunking, indexing) is language-agnostic and
//! driven by the config.

pub mod typescript;

use std::path::Path;
use std::sync::OnceLock;

use tree_sitter::Query;
use tree_sitter_language::LanguageFn;

use crate::chunk::ChunkKind;
use crate::error::{Error, Result};

/// Languages spelunk can chunk. TSX is its own variant because tree-sitter
/// ships it as a separate grammar (JSX changes the parse).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    TypeScript,
    Tsx,
}

impl Language {
    pub fn name(self) -> &'static str {
        match self {
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
        }
    }
}

/// Everything the chunker needs to know about one language.
///
/// Rust note: these live in `static` items (see [`REGISTRY`]), so references
/// to them are `&'static` — they are baked into the binary and live for the
/// whole program, which is why we can hand them around freely without
/// worrying about ownership.
pub struct LanguageConfig {
    pub language: Language,
    /// Lowercase file extensions, without the dot.
    pub extensions: &'static [&'static str],
    /// The grammar entry point exported by the tree-sitter grammar crate.
    pub grammar: LanguageFn,
    /// A tree-sitter query whose patterns each capture one definition as
    /// `@definition` and its identifier as `@name`. This query *is* the
    /// per-language chunking rule set.
    pub chunk_query: &'static str,
    /// Maps a captured node's kind (e.g. `"class_declaration"`) to a chunk kind.
    pub kind_for_node: fn(&str) -> ChunkKind,
    /// Lazily compiled form of `chunk_query`. `OnceLock::new()` is `const`,
    /// so this can sit inside a `static`.
    compiled_query: OnceLock<Query>,
}

// Manual impl because fn pointers and the compiled `Query` have no useful
// `derive(Debug)` output; the language tag identifies the config.
impl std::fmt::Debug for LanguageConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanguageConfig")
            .field("language", &self.language)
            .field("extensions", &self.extensions)
            .finish_non_exhaustive()
    }
}

impl LanguageConfig {
    pub const fn new(
        language: Language,
        extensions: &'static [&'static str],
        grammar: LanguageFn,
        chunk_query: &'static str,
        kind_for_node: fn(&str) -> ChunkKind,
    ) -> Self {
        Self {
            language,
            extensions,
            grammar,
            chunk_query,
            kind_for_node,
            compiled_query: OnceLock::new(),
        }
    }

    /// The grammar in the form tree-sitter's runtime API wants.
    pub fn ts_language(&self) -> tree_sitter::Language {
        self.grammar.into()
    }

    /// Compile the chunk query once and cache it for the process lifetime.
    pub fn query(&self) -> Result<&Query> {
        if let Some(q) = self.compiled_query.get() {
            return Ok(q);
        }
        let q =
            Query::new(&self.ts_language(), self.chunk_query).map_err(|source| Error::Query {
                language: self.language.name(),
                source,
            })?;
        // If two threads raced here, one compilation is discarded. Harmless.
        Ok(self.compiled_query.get_or_init(|| q))
    }
}

/// All registered languages. Order does not matter; extensions must not overlap.
pub static REGISTRY: &[&LanguageConfig] = &[&typescript::TYPESCRIPT, &typescript::TSX];

/// Look up the language config for a file, by extension (lowercased).
pub fn config_for_path(path: &Path) -> Option<&'static LanguageConfig> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    REGISTRY
        .iter()
        .find(|c| c.extensions.contains(&ext.as_str()))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_registered_query_compiles() {
        for config in REGISTRY {
            let query = config.query().expect("chunk query must compile");
            // The chunker relies on these two capture names existing.
            assert!(query.capture_index_for_name("definition").is_some());
            assert!(query.capture_index_for_name("name").is_some());
        }
    }

    #[test]
    fn extension_lookup() {
        assert_eq!(
            config_for_path(Path::new("src/a.ts")).map(|c| c.language),
            Some(Language::TypeScript)
        );
        assert_eq!(
            config_for_path(Path::new("src/A.TSX")).map(|c| c.language),
            Some(Language::Tsx)
        );
        assert!(config_for_path(Path::new("src/a.rs")).is_none());
        assert!(config_for_path(Path::new("Makefile")).is_none());
    }
}
