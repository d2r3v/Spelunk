# spelunk implementation plan

PR-sized steps mapped to the milestone order. Each step lists the crate APIs
it leans on and the Rust concepts worth learning at that point (coming from
TypeScript/Python/Java). Ground rules: synchronous Rust throughout, no
feature scaffolded before its milestone, spelunk-bench is the acceptance
gate from M2 onward.

## Milestone 1 — walk + chunk TypeScript ✅ (done)

**PR 1: workspace + walking + chunking + CLI printout** *(shipped as the
initial commit)*

- Crates: `ignore::WalkBuilder` (`require_git(false)`), `rayon`
  `par_iter().map_init(...)`, `tree_sitter::{Parser, Query, QueryCursor}`,
  `tree-sitter-typescript`, `clap` derive, `thiserror`/`anyhow`, `serde`.
- Rust concepts learned here (annotated in the code):
  - **Ownership & borrowing**: `Chunk` owns its `String`s; the chunker
    borrows `&str` slices of the source and only allocates at chunk
    boundaries. `LineIndex::slice` returning `&'s str` ties the returned
    borrow to the source's lifetime — that `'s` is a *lifetime parameter*.
  - **`Result` + `?` + error enums**: `thiserror` in the library,
    `anyhow::Context` at the CLI edge. Library code never panics; the
    workspace denies `clippy::unwrap_used` outside tests.
  - **`Send` vs `Sync` with rayon**: `tree_sitter::Parser` is `Send` but not
    shareable, hence `map_init` (one parser per worker) instead of a shared
    parser behind a lock.
  - **`static` data + `OnceLock`**: language configs are `&'static`, queries
    compiled lazily once per process.
  - **Streaming iterators**: tree-sitter's `QueryMatches` borrows from its
    cursor, so it can't be a std `Iterator` — `while let Some(m) =
    matches.next()`.

## Milestone 2 — tantivy BM25, end to end (ship lexical-only spelunk)

**PR 2: index persistence skeleton.** `.spelunk/` layout from DESIGN.md §1:
manifest read/write (`serde_json`), `chunks.bin` sidecar
(`bincode::serde::encode_to_vec` / `decode_from_slice`), atomic
write-tmp-then-rename helper, advisory lock file. `spelunk status` becomes
real.
- New deps: `bincode` (v2, serde feature), `fs2` (or `std` file locking once
  stable) for the lock.
- Rust concepts: `From`/`TryFrom` for version gating; the newtype pattern
  (`ChunkId(String)` or `[u8; 32]`); `Drop` for lock release (RAII — Java
  try-with-resources, but enforced by the compiler).

**PR 3: tantivy indexing.** Schema from DESIGN.md §1 (`chunk_id` STRING
stored, `path`/`name`/`body` TEXT with a code tokenizer that splits
camelCase/snake_case). Build during `spelunk index`.
- APIs: `tantivy::schema::SchemaBuilder`, `Index::create_in_dir`,
  `IndexWriter::{add_document, delete_term, commit}`, custom
  `tantivy::tokenizer::Tokenizer` impl.
- Rust concepts: trait objects vs generics (tantivy's tokenizer registry),
  builder pattern.

**PR 4: query path + first ship.** `spelunk "query"` → `QueryParser` over
`name`+`body`+`path`, top-k, snippet from stored chunk metadata; `--json`
output (ranked chunks: path, span, score, snippet). Auto-index on first
query (no index → build; stale check comes in M5). Tag `v0.1.0`.
- Acceptance: spelunk-bench lexical baseline recorded — every later
  milestone must beat or explain itself against this number.

## Milestone 3 — embeddings + vector search

**PR 5: embedding pipeline.** `fastembed` with a BGE-small-class model,
auto-download on first use with an `indicatif` progress bar; embed
`Chunk::embedding_text()` in batches.
- APIs: `fastembed::TextEmbedding::try_new(InitOptions)`, `.embed(batch)`.
- Risk item to measure immediately: ONNX session init time vs the <1 s warm
  budget (if it blows the budget, embed only at index time and consider a
  smaller query-side path — measure first).
- Rust concepts: crate features (fastembed pulls `ort`/ONNX Runtime; check
  what its default features drag in), `Vec<f32>` memory layout.

**PR 6: vector store + search.** Per DESIGN.md §1: flat `vectors.f32`
(L2-normalized rows, row-map in `chunks.bin`), brute-force dot-product
top-k. `memmap2::Mmap` for reads.
- Rust concepts: `unsafe` at a boundary (`Mmap` construction) and how to
  wrap it in a safe API; `bytemuck`/manual `f32` ↔ bytes with explicit
  little-endian layout; iterator `fold`/`select_nth_unstable` for top-k.
- usearch/HNSW deliberately deferred behind a feature flag — see DESIGN.md
  §1 rationale. Only revisit if bench latency demands it.

## Milestone 4 — RRF fusion + `--json` contract freeze

**PR 7:** run BM25 and vector search, fuse with
`score(c) = Σᵢ 1/(60 + rankᵢ(c))`, k=60 hardcoded (constant in one place,
config plumbing later). Freeze the `--json` schema and document it in the
README (spelunk-bench consumes it; treat changes as breaking from here).
- Acceptance: bench shows hybrid ≥ lexical-only on the query set; publish
  both numbers in the README.

## Milestone 5 — incremental indexing (non-negotiable feature)

**PR 8:** the algorithm in DESIGN.md §3: mtime+size fast path → `blake3`
file hash → per-chunk `blake3` content ids → set diff; only new ids
embedded; tantivy `delete_term` + vector tombstones + compaction;
`spelunk index --full` escape hatch; auto-index-if-stale before every query.
- APIs: `blake3::Hasher`, `std::fs::Metadata::modified`.
- Rust concepts: `HashMap`/`HashSet` diffing idioms, `std::time::SystemTime`
  vs unix millis.
- Acceptance (bench-measured): editing 1 file in a 10k-file repo re-embeds
  only that file's changed chunks; `spelunk index` twice in a row does zero
  embedding work.

## Milestone 6 — Python + Go

**PR 9 / PR 10:** one PR per language, each exactly one new module per the
language-layer contract (`language/python.rs`, `language/go.rs`): grammar
crate + chunk query + kind mapping + extensions, fixture files, integration
tests. Chunking rules per DESIGN.md §2 (decorators/receivers included in
spans). If the "one module" promise doesn't survive contact with Python's
grammar, fix the abstraction in the same PR.

## Milestone 7 — spelunk-mcp

**PR 11:** MCP server over stdio exposing `search_code` (query → the frozen
`--json` chunk schema) and `index_status`. Evaluate the official `rmcp` SDK
first; it is async/tokio — an accepted, dependency-forced exception to the
no-async rule, confined to the `spelunk-mcp` binary (core stays sync).
Fallback if rmcp fights: hand-rolled JSON-RPC over stdin/stdout, which MCP's
stdio transport makes feasible. One-command setup documented for Claude
Code (`claude mcp add spelunk -- spelunk-mcp`).
- Rust concepts (only if rmcp): just enough tokio to run one async runtime
  at the binary edge.

## Cross-cutting

- **CI** (exists): Linux + macOS — fmt, clippy `-D warnings`, tests. Add a
  Windows job once tantivy/fastembed land to catch native-dep breakage where
  it actually happens (the dev machine is Windows).
- **Releases** (exists): tag `v*` → binaries for linux-x86_64,
  macos-x86_64/aarch64.
- **Bench discipline**: every milestone from M2 ends by running spelunk-bench
  and recording the number in the PR description. A milestone that can't be
  measured isn't done.
