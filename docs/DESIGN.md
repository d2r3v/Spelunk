# spelunk design

spelunk is a hybrid code retrieval engine: BM25 (tantivy) and embedding search
(fastembed + a vector index) over tree-sitter chunks, fused with reciprocal
rank fusion. This document covers the three things that must stay stable while
the code grows: the on-disk index layout, the chunking rules, and the
incremental-update algorithm. Milestone status: only walking + chunking exist
today; everything else here is the design the milestones build toward.

Guiding constraints, in priority order:

1. **Correct and measurable** — spelunk-bench is the arbiter. Every design
   choice should be checkable by a benchmark or a test.
2. **Zero config, no daemon** — first run indexes, warm queries answer < 1 s.
3. **Boring Rust** — synchronous, explicit, minimal dependencies.

## 1. Index layout on disk

Everything lives under `.spelunk/` at the repo root (users add it to
`.gitignore`; the walker hard-excludes it regardless).

```
.spelunk/
├── manifest.json      # small, human-readable: versions + file table
├── chunks.bin         # chunk metadata sidecar (bincode)
├── tantivy/           # tantivy-managed BM25 index directory
├── vectors.f32        # embedding matrix, row-per-chunk, mmap-friendly
└── lock               # advisory lock: one writer at a time
```

### manifest.json

The source of truth for *what has been indexed*. Read at startup of every
command; small enough (one entry per file) that JSON + pretty printing is
fine and greppable when debugging.

```json
{
  "format_version": 1,
  "spelunk_version": "0.3.0",
  "embedding_model": "bge-small-en-v1.5",
  "embedding_dim": 384,
  "files": {
    "src/auth.ts": {
      "mtime_ms": 1751871234000,
      "size": 2311,
      "blake3": "9f2c…",
      "chunk_ids": ["c41a…", "77b0…"]
    }
  }
}
```

- `format_version` gates compatibility: on mismatch, delete `.spelunk/` and
  rebuild. No migrations — an index is a cache, never precious data.
- `embedding_model`/`embedding_dim` changing also forces a rebuild (vectors
  from different models are not comparable).
- Paths are relative and `/`-separated on every platform.

### chunks.bin — the metadata store (sidecar file, not redb)

One bincode-serialized `Vec<StoredChunk>` (chunk id, path, span, kind, name,
text). Loaded fully into a `HashMap<ChunkId, StoredChunk>` at query time.

Why a sidecar file instead of redb:

- At the design ceiling (~100k chunks) this is tens of MB — a full load is a
  single sequential read, well inside the warm-query budget. redb's value is
  transactions and point lookups on data too big to load; we have neither
  problem.
- Zero extra dependencies, no schema evolution machinery, no B-tree tuning.
  One `serde` derive.
- Writes are atomic by construction: write `chunks.bin.tmp`, then rename.
  Combined with the manifest being written *last*, a crashed indexing run
  leaves a stale-but-consistent index, never a corrupt one.
- Trivially debuggable (`spelunk status --verbose` can dump it).

Revisit only if spelunk-bench shows metadata load dominating warm-query
latency; the `ChunkStore` type is the seam where redb would slot in.

### tantivy/

A normal tantivy index directory. Schema (milestone 2):

| field        | type   | indexed | stored | notes                          |
|--------------|--------|---------|--------|--------------------------------|
| `chunk_id`   | STRING | yes     | yes    | term key for deletes           |
| `path`       | TEXT   | yes     | yes    | tokenized so path words match  |
| `name`       | TEXT   | yes     | yes    | definition name, boosted       |
| `body`       | TEXT   | yes     | no     | chunk text, code-aware tokens  |
| `kind`       | STRING | yes     | yes    | future filtering               |

Body text goes through a code-friendly tokenizer (split `snake_case` /
`camelCase`; exact scheme decided in M2 and validated by spelunk-bench).

### vectors.f32 (+ vector index strategy)

Milestone 3 starts with the **brute-force fallback as the primary plan**, not
the fallback: a flat little-endian `f32` matrix, one L2-normalized row per
chunk, row order defined by a `Vec<ChunkId>` stored in `chunks.bin`. Search =
mmap the file, dot-product every row against the query vector, take top-k.
At 100k chunks × 384 dims that is ~150 MB of sequential reads worst case
(~57 M multiply-adds) — comfortably sub-100 ms on anything modern, exact
(no recall loss — a cleaner baseline for spelunk-bench), and ~100 lines of
code with zero native build risk.

usearch (HNSW) is the *upgrade path*, behind a cargo feature, adopted only if
the bench shows vector search dominating latency at real repo sizes. Rationale
for demoting it: the Rust crate builds C++ via cmake and has a record of
platform friction — including on Windows, the primary dev machine — and HNSW
adds a recall/latency tradeoff that muddies benchmark ground truth. This is a
deliberate deviation from the original architecture sketch, flagged early.

Deletions: tombstone rows (zero vector + id removed from the row map),
compacted by rewriting the file when tombstones exceed 20% of rows.

### lock

`fs2`-style advisory file lock taken by any command that writes. Readers
don't lock (they read a consistent snapshot because writers replace files
atomically and write the manifest last).

## 2. Chunking rules

Chunking is split into a language-agnostic policy (in `chunk.rs`) and a
per-language definition of "what is a definition" (a tree-sitter query in
`language/<lang>.rs`). **Adding a language = one module**: grammar crate +
extensions + chunk query + node-kind→chunk-kind mapping.

### Policy (all languages)

- A **chunk** is a contiguous span of one file: `path`, 1-based inclusive
  `start_line..end_line`, `kind`, optional `name`, exact `text`.
- Every definition captured by the language query becomes a chunk. An
  `export`/decorator-style wrapper node is included in the span, and so are
  comments sitting directly on top of the definition (doc comments), so
  retrieval sees documentation together with the code it documents.
- Definitions nested inside functions/methods are **not** split out; the
  outer function is the retrieval unit.
- A class that fits within `MAX_CHUNK_LINES` (150) is one chunk. A larger
  class becomes a *header* chunk (declaration + fields, up to the first
  method) plus one chunk per method, named `Class.method`. Known loss:
  comments between methods not attached to the next method are dropped when
  a class splits.
- Top-level code outside all definitions (imports, constants, config
  objects) becomes `module` chunks, one per contiguous gap, blank edges
  trimmed.
- Chunks longer than `MAX_CHUNK_LINES` split into consecutive line windows
  sharing kind/name. Line spans stay truthful — a benchmark can always map a
  chunk back to source.
- Parsing never hard-fails: tree-sitter yields a tree with error nodes;
  uncaptured code falls into module chunks. Non-UTF-8 or unreadable files
  are skipped and reported, never fatal.
- **Embedding text ≠ chunk text**: what gets embedded (M3) is
  `"{path} {qualified name}\n{chunk text}"` so location context is in the
  vector. The stored chunk text stays verbatim source.

### TypeScript / TSX (launch)

Captured as definitions: `function`/generator declarations, classes
(incl. abstract), interfaces, enums, type aliases, `method_definition`,
`const/let/var x = () => {}` and `= function () {}` bindings, and class
property functions (`handle = () => {}`). TSX uses the separate TSX grammar;
same query.

### Python (milestone 6)

`function_definition`, `class_definition`, decorated definitions (span grows
to include decorators — same mechanism as TS `export`). Methods = functions
nested directly in a class. Module-level assignments and imports become
module chunks via the generic gap rule.

### Go (milestone 6)

`function_declaration`, `method_declaration` (named
`Receiver.method`), `type_declaration` (structs/interfaces), top-level
`const`/`var` blocks. Same generic policy otherwise.

## 3. Incremental update algorithm

Non-negotiable requirement: only changed chunks are re-embedded (embedding is
the expensive step by orders of magnitude).

Chunk identity is content-addressed:

```
chunk_id = blake3(rel_path ‖ kind ‖ qualified_name ‖ chunk_text)
```

Including `rel_path` means a moved file re-embeds (correct: path is part of
the embedding text). Including span *lines* is deliberately avoided: code
shifted down by an added import keeps its chunk_id — only the stored line
span is refreshed.

Update pass (`spelunk index`, also run implicitly before a query):

```
1. Walk the repo → current file list.
2. Deleted files: in manifest but not on disk → all their chunk_ids marked
   for removal.
3. Per file, fast path: (mtime_ms, size) equal to manifest entry → skip file
   entirely. (mtime is a hint, never truth.)
4. Slow path: read file, blake3 the bytes. Hash equal to manifest → touch
   manifest mtime, done (mtime changed but content didn't — git checkout,
   touch, CI).
5. Content changed → re-chunk the file. Diff new chunk_id set against the
   manifest's set for that file:
     - unchanged ids: keep; update stored line spans/text metadata if the
       chunk moved (cheap, no re-embed);
     - new ids: index into tantivy + embed + append to vector store;
     - vanished ids: delete from tantivy (delete_term on chunk_id),
       tombstone in vector store, remove from chunks.bin.
6. Batch all embedding work across files into one fastembed batch run
   (throughput), with a progress bar when interactive.
7. Commit order: tantivy commit → vectors.f32 (+ tmp/rename) → chunks.bin
   (tmp/rename) → manifest.json (tmp/rename) last. A crash before the
   manifest write leaves the old manifest pointing only at chunk_ids that
   still exist in the (append-only or atomically-replaced) stores, so the
   index stays consistent; the next run redoes the interrupted work.
```

Edge cases pinned down now:

- **Same chunk text in two files** → different ids (path is hashed). Same
  text twice in one file → same id; store dedups, both spans recorded on the
  chunk record (rare; acceptable).
- **Renamed file** → old ids removed, new ids embedded. Correctness over
  cleverness; renames are rare relative to edits.
- **Clock skew / sub-second mtimes** → step 4 catches every false "changed";
  false "unchanged" requires equal mtime *and* size with different content,
  which git-driven workflows don't produce silently. Accepted risk, and
  `spelunk index --full` (M5) bypasses the fast path.
- **Concurrent runs** → writer lock; a second `spelunk index` waits or
  no-ops.

## 4. Query path (for context)

M2: query → tantivy BM25 top-50. M3+: same query embedded → vector top-50.
M4: reciprocal rank fusion, `score(c) = Σ 1/(60 + rank_i(c))`, k=60 fixed
until configurable; ranked chunks (path, span, score, snippet) to terminal or
`--json`. Warm-query budget: manifest+chunks load ≤ 150 ms, both searches
≤ 300 ms combined, leaving headroom under 1 s including process start and
model load (the ONNX session is the risk item; measured in M3).
