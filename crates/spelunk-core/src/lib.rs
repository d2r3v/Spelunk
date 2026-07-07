//! spelunk-core: the indexing and query engine behind the `spelunk` CLI and
//! `spelunk-mcp` server.
//!
//! Milestone 1 scope: walk a repository (respecting ignore rules) and chunk
//! source files into definition-level pieces via tree-sitter. Indexing,
//! embeddings, and search build on top of these chunks in later milestones.

pub mod chunk;
pub mod error;
pub mod language;
pub mod walk;

pub use chunk::{Chunk, ChunkKind, ChunkOutcome, chunk_files, chunk_source};
pub use error::{Error, Result};
pub use language::{Language, LanguageConfig};
pub use walk::{SourceFile, walk_source_files};
