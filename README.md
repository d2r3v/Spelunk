# spelunk

Hybrid code search for your repository: lexical (BM25) + semantic (embeddings),
fused with reciprocal rank fusion. One binary, zero config, no daemon. Built to
be a retrieval backend for coding agents via MCP — and to be *measured*, by
[spelunk-bench](#) <!-- TODO: link once the bench repo is public -->.

```console
$ spelunk "where is rate limiting implemented?"
src/rateLimit.ts:43-51  function  rateLimit   0.94
src/rateLimit.ts:8-36   class     TokenBucket 0.87
...
```

> **Status: pre-alpha.** Milestone 1 (walk + chunk TypeScript) is done; search
> lands at milestone 2. See [docs/PLAN.md](docs/PLAN.md).

## How it works

- Walks the current repo (respects `.gitignore`), chunks code at
  function/method/class granularity with tree-sitter.
- Indexes chunks twice: BM25 via tantivy, and embeddings (BGE-small via ONNX,
  downloaded on first run) in a vector index.
- Queries both, merges rankings with reciprocal rank fusion.
- Incremental: only changed chunks (blake3-verified) are re-embedded.
- Everything lives in `.spelunk/` inside your repo (add it to `.gitignore`).

Design details: [docs/DESIGN.md](docs/DESIGN.md).

## Install

<!-- TODO once released: curl one-liner + cargo install spelunk-cli -->
Build from source for now:

```console
$ cargo build --release
$ target/release/spelunk index --print-chunks
```

## Usage

```console
$ spelunk "query"            # search (auto-indexes on first run)
$ spelunk --json "query"     # ranked chunks as JSON: path, lines, score, snippet
$ spelunk index              # (re)index explicitly
$ spelunk status             # index freshness and stats
```

## MCP server (for coding agents)

`spelunk-mcp` exposes `search_code` and `index_status` tools.
<!-- TODO milestone 7: one-command Claude Code setup -->

## Supported languages

TypeScript/TSX at launch; Python and Go next. Adding a language is one module:
see `crates/spelunk-core/src/language/`.

## Non-goals

Call graphs, file watchers/daemons, editor extensions, rerankers, UIs, cloud
anything.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in spelunk by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
