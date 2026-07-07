//! Integration test: walk + chunk the checked-in fixture repo and assert
//! known definitions land in known files with sane spans.

// Integration-test helper fns are not inside #[cfg(test)], so clippy.toml's
// allow-unwrap-in-tests does not reach them.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use spelunk_core::{Chunk, ChunkKind, Language, chunk_files, walk_source_files};

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/simple-ts")
}

fn chunk_fixture() -> Vec<Chunk> {
    let files = walk_source_files(&fixture_root()).unwrap();
    let outcome = chunk_files(&files);
    assert!(
        outcome.skipped.is_empty(),
        "no fixture file should be skipped: {:?}",
        outcome.skipped
    );
    outcome.chunks
}

fn find<'c>(chunks: &'c [Chunk], name: &str) -> &'c Chunk {
    chunks
        .iter()
        .find(|c| c.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("expected a chunk named {name}"))
}

#[test]
fn walks_only_source_files_and_respects_gitignore() {
    let files = walk_source_files(&fixture_root()).unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "src/auth.ts",
            "src/index.tsx",
            "src/rateLimit.ts",
            "src/util.ts",
        ],
        "dist/ must be ignored, package.json is not a source file"
    );
}

#[test]
fn known_queries_map_to_known_files() {
    let chunks = chunk_fixture();

    let rate_limit = find(&chunks, "rateLimit");
    assert_eq!(rate_limit.path, "src/rateLimit.ts");
    assert_eq!(rate_limit.kind, ChunkKind::Function);
    assert!(rate_limit.text.contains("bucket.tryTake()"));

    let login = find(&chunks, "login");
    assert_eq!(login.path, "src/auth.ts");
    assert!(login.text.starts_with("export async function login"));

    let manager = find(&chunks, "SessionManager");
    assert_eq!(manager.path, "src/auth.ts");
    assert_eq!(manager.kind, ChunkKind::Class);
    // Small class: one chunk, methods not split out.
    assert!(manager.text.contains("refresh(sessionId: string)"));

    let badge = find(&chunks, "StatusBadge");
    assert_eq!(badge.path, "src/index.tsx");
    assert_eq!(badge.language, Language::Tsx);

    let hash = find(&chunks, "hashPassword");
    assert_eq!(hash.path, "src/util.ts");
    assert_eq!(hash.kind, ChunkKind::Function);
}

#[test]
fn chunks_are_well_formed() {
    let chunks = chunk_fixture();
    assert!(!chunks.is_empty());
    for chunk in &chunks {
        assert!(chunk.start_line >= 1, "{chunk:?}");
        assert!(chunk.start_line <= chunk.end_line, "{chunk:?}");
        assert!(!chunk.text.trim().is_empty(), "{chunk:?}");
        assert!(
            !chunk.path.starts_with("dist/"),
            "ignored file was chunked: {chunk:?}"
        );
        assert!(!chunk.path.contains('\\'), "paths must be /-separated");
    }
}
