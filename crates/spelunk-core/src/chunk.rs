//! Chunking: split source files into definition-level chunks using
//! tree-sitter.
//!
//! Policy (language-agnostic; the per-language part is the chunk query in
//! `language/*.rs`):
//!
//! - Every definition captured by the language's chunk query becomes a chunk.
//!   Comments sitting directly on top of a definition (doc comments) are
//!   included in its chunk.
//! - Definitions nested inside functions/methods are *not* split out — the
//!   outer function is the retrieval unit.
//! - A class that fits in [`MAX_CHUNK_LINES`] is one chunk. A larger class is
//!   split into a header chunk (declaration + fields, up to the first method)
//!   plus one chunk per method, named `Class.method`. Comments between
//!   methods that are *not* attached to the next method are dropped when a
//!   class splits (known, accepted loss).
//! - Top-level code outside any definition (imports, constants, config
//!   objects) is grouped into `Module` chunks.
//! - Any chunk longer than [`MAX_CHUNK_LINES`] is split into consecutive
//!   line windows. Line spans are always truthful: 1-based, inclusive.
//! - Chunking never hard-fails on bad syntax: tree-sitter produces a tree
//!   with error nodes, and anything uncaptured falls into `Module` chunks.

use std::fs;

use rayon::prelude::*;
use streaming_iterator::StreamingIterator;
use tree_sitter::{Node, Parser, QueryCursor};

use crate::error::{Error, Result};
use crate::language::{Language, LanguageConfig};
use crate::walk::SourceFile;

/// Chunks longer than this are split. Roughly sized so a chunk stays useful
/// as a retrieval unit and mostly survives embedding-model truncation (M3).
pub const MAX_CHUNK_LINES: usize = 150;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChunkKind {
    Function,
    Method,
    Class,
    Interface,
    Enum,
    TypeAlias,
    /// Top-level code that is not part of any captured definition.
    Module,
}

/// One retrieval unit: a contiguous span of one source file.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Chunk {
    /// Relative, `/`-separated path of the file this chunk came from.
    pub path: String,
    pub language: Language,
    pub kind: ChunkKind,
    /// Definition name (`login`, `SessionManager.refresh`); `None` for module chunks.
    pub name: Option<String>,
    /// 1-based, inclusive.
    pub start_line: usize,
    /// 1-based, inclusive.
    pub end_line: usize,
    /// Exact source text of the span (whole lines).
    pub text: String,
}

impl Chunk {
    /// First non-empty line, trimmed — used as the signature in output.
    pub fn signature(&self) -> &str {
        self.text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("")
    }

    /// The text that will be embedded in milestone 3. Contract: file path and
    /// qualified name are prepended so the vector carries location context.
    pub fn embedding_text(&self) -> String {
        match &self.name {
            Some(name) => format!("{} {}\n{}", self.path, name, self.text),
            None => format!("{}\n{}", self.path, self.text),
        }
    }
}

/// A file that was walked but could not be chunked (unreadable, non-UTF-8).
#[derive(Debug)]
pub struct SkippedFile {
    pub rel_path: String,
    pub reason: String,
}

/// Result of chunking a set of files. Per-file problems skip that file
/// instead of failing the run.
#[derive(Debug, Default)]
pub struct ChunkOutcome {
    pub chunks: Vec<Chunk>,
    pub skipped: Vec<SkippedFile>,
}

/// Chunk many files in parallel with rayon.
///
/// Rust note: `tree_sitter::Parser` is `Send` (movable across threads) but
/// holds mutable parse state, so threads can't share one. `map_init` gives
/// each rayon worker its own `Parser`, reused across the files that worker
/// processes — no locks, no per-file allocation of a parser.
pub fn chunk_files(files: &[SourceFile]) -> ChunkOutcome {
    let results: Vec<std::result::Result<Vec<Chunk>, SkippedFile>> = files
        .par_iter()
        .map_init(Parser::new, |parser, file| {
            let skip = |reason: String| SkippedFile {
                rel_path: file.rel_path.clone(),
                reason,
            };
            let bytes = fs::read(&file.abs_path).map_err(|e| skip(format!("read failed: {e}")))?;
            let source =
                String::from_utf8(bytes).map_err(|_| skip("not valid UTF-8".to_string()))?;
            chunk_source_with(parser, file.config, &file.rel_path, &source)
                .map_err(|e| skip(e.to_string()))
        })
        .collect();

    let mut outcome = ChunkOutcome::default();
    for result in results {
        match result {
            Ok(chunks) => outcome.chunks.extend(chunks),
            Err(skipped) => outcome.skipped.push(skipped),
        }
    }
    outcome
}

/// Chunk a single source string. Convenience wrapper that owns its parser;
/// batch callers should go through [`chunk_files`].
pub fn chunk_source(config: &LanguageConfig, rel_path: &str, source: &str) -> Result<Vec<Chunk>> {
    let mut parser = Parser::new();
    chunk_source_with(&mut parser, config, rel_path, source)
}

fn chunk_source_with(
    parser: &mut Parser,
    config: &LanguageConfig,
    rel_path: &str,
    source: &str,
) -> Result<Vec<Chunk>> {
    parser
        .set_language(&config.ts_language())
        .map_err(|source| Error::Grammar {
            language: config.language.name(),
            source,
        })?;

    let lines = LineIndex::new(source);
    if lines.count == 0 {
        return Ok(Vec::new());
    }

    let defs = match parser.parse(source, None) {
        Some(tree) => collect_definitions(config, tree.root_node(), source)?,
        // `parse` only returns None on cancellation/timeout, which we never
        // enable — but if it happens, degrade to module chunks.
        None => Vec::new(),
    };

    Ok(assemble_chunks(
        rel_path,
        config.language,
        source,
        &lines,
        defs,
    ))
}

/// A captured definition, before assembly into chunks.
#[derive(Debug)]
struct Def {
    start_byte: usize,
    end_byte: usize,
    start_line: usize,
    end_line: usize,
    kind: ChunkKind,
    name: String,
}

impl Def {
    fn contains(&self, other: &Def) -> bool {
        self.start_byte <= other.start_byte && other.end_byte <= self.end_byte
    }

    fn line_count(&self) -> usize {
        self.end_line - self.start_line + 1
    }
}

fn collect_definitions(config: &LanguageConfig, root: Node, source: &str) -> Result<Vec<Def>> {
    let query = config.query()?;
    // These capture names are part of the LanguageConfig contract and are
    // checked for every registered language by a unit test, so failure here
    // is a programming error, not a runtime condition.
    let def_idx = query
        .capture_index_for_name("definition")
        .expect("chunk query must have a @definition capture");
    let name_idx = query
        .capture_index_for_name("name")
        .expect("chunk query must have a @name capture");

    let mut defs: Vec<Def> = Vec::new();
    let mut cursor = QueryCursor::new();
    // Rust note: tree-sitter's matches() is a *streaming* iterator (each item
    // borrows from the cursor), so it can't be a std Iterator — hence the
    // `while let` + `StreamingIterator` import instead of a `for` loop.
    let mut matches = cursor.matches(query, root, source.as_bytes());
    while let Some(m) = matches.next() {
        let mut def_node: Option<Node> = None;
        let mut name = String::new();
        for capture in m.captures {
            if capture.index == def_idx {
                def_node = Some(capture.node);
            } else if capture.index == name_idx {
                name = source[capture.node.byte_range()].to_string();
            }
        }
        let Some(node) = def_node else { continue };

        // Kind comes from the captured node itself...
        let kind = (config.kind_for_node)(node.kind());
        // ...but the chunk span grows to include an `export` wrapper, so the
        // chunk text keeps the `export` keyword.
        let node = match node.parent() {
            Some(parent) if parent.kind() == "export_statement" => parent,
            _ => node,
        };

        // Doc comments immediately above a definition belong to it: walk
        // preceding comment siblings while they touch the line above.
        let mut start_node = node;
        while let Some(prev) = start_node.prev_sibling() {
            if prev.kind() == "comment"
                && prev.end_position().row + 1 == start_node.start_position().row
            {
                start_node = prev;
            } else {
                break;
            }
        }

        let (start_line, _) = node_line_span(&start_node);
        let (_, end_line) = node_line_span(&node);
        defs.push(Def {
            start_byte: start_node.start_byte(),
            end_byte: node.end_byte(),
            start_line,
            end_line,
            kind,
            name,
        });
    }

    // Sort outer-before-inner so the containment pass below can use a single
    // forward scan: by start ascending, then wider (later end) first.
    defs.sort_by(|a, b| {
        a.start_byte
            .cmp(&b.start_byte)
            .then(b.end_byte.cmp(&a.end_byte))
    });
    // One declaration statement can bind several functions
    // (`const a = () => 1, b = () => 2;`) and would be captured once per
    // binding with an identical span; keep the first.
    defs.dedup_by(|a, b| a.start_byte == b.start_byte && a.end_byte == b.end_byte);
    Ok(defs)
}

/// tree-sitter positions are 0-based; convert to a 1-based inclusive line
/// span. A node ending at column 0 stops *before* that line.
fn node_line_span(node: &Node) -> (usize, usize) {
    let start = node.start_position().row + 1;
    let end_pos = node.end_position();
    let mut end = end_pos.row + 1;
    if end_pos.column == 0 && end > start {
        end -= 1;
    }
    (start, end)
}

/// A top-level definition plus the nested definitions we chose to keep
/// (methods of a class). Everything else nested is dropped: the outer
/// definition is the chunk.
struct DefTree {
    def: Def,
    children: Vec<Def>,
}

fn assemble_chunks(
    rel_path: &str,
    language: Language,
    source: &str,
    lines: &LineIndex,
    defs: Vec<Def>,
) -> Vec<Chunk> {
    // Pass 1: nesting. Syntax-tree spans either nest or are disjoint, and
    // defs are sorted outer-first, so "the last top-level def" is the only
    // possible container for the current one.
    let mut tree: Vec<DefTree> = Vec::new();
    for def in defs {
        match tree.last_mut() {
            Some(top) if top.def.contains(&def) => {
                let inside_kept_child = top.children.last().is_some_and(|c| c.contains(&def));
                if top.def.kind == ChunkKind::Class && !inside_kept_child {
                    // A method (or property function) directly inside a
                    // top-level class: kept, emitted as its own chunk if the
                    // class is split.
                    top.children.push(def);
                }
                // Otherwise it's nested inside a function/method (a closure,
                // a local class): dropped — the enclosing definition is the
                // chunk.
            }
            _ => tree.push(DefTree {
                def,
                children: Vec::new(),
            }),
        }
    }

    let mut chunks: Vec<Chunk> = Vec::new();

    // Pass 2: emit definition chunks.
    for DefTree { def, children } in &tree {
        let split_class = def.kind == ChunkKind::Class
            && !children.is_empty()
            && def.line_count() > MAX_CHUNK_LINES;

        if split_class {
            // Header: class declaration + fields, up to the first method.
            let header_end = children[0].start_line.saturating_sub(1);
            if header_end >= def.start_line {
                push_split(
                    &mut chunks,
                    rel_path,
                    language,
                    source,
                    lines,
                    ChunkKind::Class,
                    Some(def.name.clone()),
                    def.start_line,
                    header_end,
                );
            }
            for child in children {
                push_split(
                    &mut chunks,
                    rel_path,
                    language,
                    source,
                    lines,
                    child.kind,
                    Some(format!("{}.{}", def.name, child.name)),
                    child.start_line,
                    child.end_line,
                );
            }
        } else {
            push_split(
                &mut chunks,
                rel_path,
                language,
                source,
                lines,
                def.kind,
                Some(def.name.clone()),
                def.start_line,
                def.end_line,
            );
        }
    }

    // Pass 3: module chunks for top-level lines not covered by any definition.
    let mut covered = vec![false; lines.count + 1]; // 1-based
    for DefTree { def, .. } in &tree {
        covered[def.start_line..=def.end_line.min(lines.count)].fill(true);
    }
    let mut gap_start: Option<usize> = None;
    for (line, &is_covered) in covered.iter().enumerate().skip(1) {
        match (gap_start, is_covered) {
            (None, false) => gap_start = Some(line),
            (Some(start), true) => {
                push_module_gap(
                    &mut chunks,
                    rel_path,
                    language,
                    source,
                    lines,
                    start,
                    line - 1,
                );
                gap_start = None;
            }
            _ => {}
        }
    }
    if let Some(start) = gap_start {
        push_module_gap(
            &mut chunks,
            rel_path,
            language,
            source,
            lines,
            start,
            lines.count,
        );
    }

    chunks.sort_by_key(|c| (c.start_line, c.end_line));
    chunks
}

/// Emit a gap as `Module` chunk(s), trimmed of blank edge lines; skipped
/// entirely if blank.
fn push_module_gap(
    chunks: &mut Vec<Chunk>,
    rel_path: &str,
    language: Language,
    source: &str,
    lines: &LineIndex,
    start: usize,
    end: usize,
) {
    let mut start = start;
    let mut end = end;
    while start <= end && lines.is_blank(source, start) {
        start += 1;
    }
    while end >= start && lines.is_blank(source, end) {
        end -= 1;
    }
    if start > end {
        return;
    }
    push_split(
        chunks,
        rel_path,
        language,
        source,
        lines,
        ChunkKind::Module,
        None,
        start,
        end,
    );
}

/// Push the span as one chunk, or several consecutive windows if it exceeds
/// [`MAX_CHUNK_LINES`]. Parts share kind and name; their line spans stay
/// truthful.
#[allow(clippy::too_many_arguments)]
fn push_split(
    chunks: &mut Vec<Chunk>,
    rel_path: &str,
    language: Language,
    source: &str,
    lines: &LineIndex,
    kind: ChunkKind,
    name: Option<String>,
    start_line: usize,
    end_line: usize,
) {
    let mut window_start = start_line;
    while window_start <= end_line {
        let window_end = end_line.min(window_start + MAX_CHUNK_LINES - 1);
        chunks.push(Chunk {
            path: rel_path.to_string(),
            language,
            kind,
            name: name.clone(),
            start_line: window_start,
            end_line: window_end,
            text: lines.slice(source, window_start, window_end).to_string(),
        });
        window_start = window_end + 1;
    }
}

/// Byte offsets of line starts, for slicing chunks out of the source by line
/// number without re-scanning.
struct LineIndex {
    starts: Vec<usize>,
    count: usize,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        for (i, byte) in source.bytes().enumerate() {
            if byte == b'\n' && i + 1 < source.len() {
                starts.push(i + 1);
            }
        }
        let count = if source.is_empty() { 0 } else { starts.len() };
        Self { starts, count }
    }

    /// Source text of the 1-based inclusive line span, including the final
    /// newline if present.
    fn slice<'s>(&self, source: &'s str, start_line: usize, end_line: usize) -> &'s str {
        let start = self.starts[start_line - 1];
        let end = if end_line < self.count {
            self.starts[end_line]
        } else {
            source.len()
        };
        &source[start..end]
    }

    fn is_blank(&self, source: &str, line: usize) -> bool {
        self.slice(source, line, line).trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language::typescript::TYPESCRIPT;

    fn chunk_ts(source: &str) -> Vec<Chunk> {
        chunk_source(&TYPESCRIPT, "src/test.ts", source).unwrap()
    }

    fn find<'c>(chunks: &'c [Chunk], name: &str) -> &'c Chunk {
        chunks
            .iter()
            .find(|c| c.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no chunk named {name} in {chunks:#?}"))
    }

    #[test]
    fn functions_and_imports() {
        let source = "\
import { x } from \"./x\";

function alpha(a: number): number {
  return a + 1;
}

export function beta(): void {
  alpha(2);
}
";
        let chunks = chunk_ts(source);

        let alpha = find(&chunks, "alpha");
        assert_eq!(alpha.kind, ChunkKind::Function);
        assert_eq!((alpha.start_line, alpha.end_line), (3, 5));
        assert!(alpha.text.starts_with("function alpha"));

        // Export wrapper is included in the chunk span and text.
        let beta = find(&chunks, "beta");
        assert_eq!((beta.start_line, beta.end_line), (7, 9));
        assert!(beta.text.starts_with("export function beta"));

        // The import line becomes a module chunk, blank lines trimmed.
        let module: Vec<_> = chunks
            .iter()
            .filter(|c| c.kind == ChunkKind::Module)
            .collect();
        assert_eq!(module.len(), 1);
        assert_eq!((module[0].start_line, module[0].end_line), (1, 1));
        assert_eq!(module[0].name, None);
    }

    #[test]
    fn const_arrow_functions_and_type_definitions() {
        let source = "\
export const clamp = (n: number, lo: number, hi: number) =>
  Math.min(hi, Math.max(lo, n));

export interface Pair {
  left: number;
  right: number;
}

export type Result = Pair | null;

enum Direction {
  Up,
  Down,
}
";
        let chunks = chunk_ts(source);
        assert_eq!(find(&chunks, "clamp").kind, ChunkKind::Function);
        assert_eq!(find(&chunks, "Pair").kind, ChunkKind::Interface);
        assert_eq!(find(&chunks, "Result").kind, ChunkKind::TypeAlias);
        assert_eq!(find(&chunks, "Direction").kind, ChunkKind::Enum);
        assert!(
            find(&chunks, "clamp")
                .text
                .starts_with("export const clamp")
        );
    }

    #[test]
    fn small_class_is_one_chunk() {
        let source = "\
class Greeter {
  private name = \"world\";

  greet(): string {
    return `hello ${this.name}`;
  }
}
";
        let chunks = chunk_ts(source);
        assert_eq!(chunks.len(), 1);
        let class = find(&chunks, "Greeter");
        assert_eq!(class.kind, ChunkKind::Class);
        assert_eq!((class.start_line, class.end_line), (1, 7));
        // No separate chunk for the method of a small class.
        assert!(
            !chunks
                .iter()
                .any(|c| c.name.as_deref() == Some("Greeter.greet"))
        );
    }

    #[test]
    fn large_class_splits_into_header_and_methods() {
        // Build a class guaranteed to exceed MAX_CHUNK_LINES.
        let body_lines = MAX_CHUNK_LINES; // per-method padding
        let mut source = String::from("export class Big {\n  private count = 0;\n\n");
        for name in ["first", "second"] {
            source.push_str(&format!("  {name}(): void {{\n"));
            for i in 0..body_lines / 2 {
                source.push_str(&format!("    this.count += {i};\n"));
            }
            source.push_str("  }\n\n");
        }
        source.push_str("}\n");

        let chunks = chunk_ts(&source);

        let header = find(&chunks, "Big");
        assert_eq!(header.kind, ChunkKind::Class);
        assert_eq!(header.start_line, 1);
        assert!(header.text.starts_with("export class Big"));
        assert!(header.text.contains("private count"));
        assert!(!header.text.contains("this.count +="));

        let first = find(&chunks, "Big.first");
        assert_eq!(first.kind, ChunkKind::Method);
        assert!(first.text.trim_start().starts_with("first(): void"));
        find(&chunks, "Big.second");
    }

    #[test]
    fn nested_functions_stay_inside_their_parent() {
        let source = "\
function outer(): number {
  const inner = () => 41;
  function innermost(): number {
    return inner() + 1;
  }
  return innermost();
}
";
        let chunks = chunk_ts(source);
        assert_eq!(chunks.len(), 1);
        let outer = find(&chunks, "outer");
        assert_eq!((outer.start_line, outer.end_line), (1, 7));
        assert!(outer.text.contains("innermost"));
    }

    #[test]
    fn oversized_function_splits_into_truthful_windows() {
        let mut source = String::from("function huge(): void {\n");
        for i in 0..(MAX_CHUNK_LINES * 2) {
            source.push_str(&format!("  console.log({i});\n"));
        }
        source.push_str("}\n");
        let total_lines = MAX_CHUNK_LINES * 2 + 2;

        let chunks = chunk_ts(&source);
        let parts: Vec<_> = chunks
            .iter()
            .filter(|c| c.name.as_deref() == Some("huge"))
            .collect();
        assert_eq!(parts.len(), 3); // 302 lines -> 150 + 150 + 2

        assert_eq!(parts[0].start_line, 1);
        for pair in parts.windows(2) {
            assert_eq!(pair[1].start_line, pair[0].end_line + 1);
        }
        assert_eq!(parts.last().unwrap().end_line, total_lines);

        // Reassembling the parts gives back the original source.
        let rebuilt: String = parts.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(rebuilt, source);
    }

    #[test]
    fn leading_doc_comments_attach_to_their_definition() {
        let source = "\
// detached file-header comment

/**
 * Checks the request budget.
 */
export function check(): boolean {
  return true;
}
";
        let chunks = chunk_ts(source);
        let check = find(&chunks, "check");
        assert_eq!((check.start_line, check.end_line), (3, 8));
        assert!(check.text.starts_with("/**"));

        // The header comment is separated by a blank line: stays a module chunk.
        let modules: Vec<_> = chunks
            .iter()
            .filter(|c| c.kind == ChunkKind::Module)
            .collect();
        assert_eq!(modules.len(), 1);
        assert_eq!((modules[0].start_line, modules[0].end_line), (1, 1));
    }

    #[test]
    fn broken_syntax_still_produces_chunks() {
        let source = "\
function ok(): number {
  return 1;
}

this is (not valid typescript at all
";
        let chunks = chunk_ts(source);
        find(&chunks, "ok");
        // The broken tail lands in a module chunk instead of being dropped.
        assert!(
            chunks
                .iter()
                .any(|c| c.kind == ChunkKind::Module && c.text.contains("not valid"))
        );
    }

    #[test]
    fn blank_and_empty_files_produce_no_chunks() {
        assert!(chunk_ts("").is_empty());
        assert!(chunk_ts("\n\n  \n").is_empty());
    }

    #[test]
    fn embedding_text_carries_path_and_name() {
        let chunks = chunk_ts("export function login(): void {}\n");
        let login = find(&chunks, "login");
        let text = login.embedding_text();
        assert!(text.starts_with("src/test.ts login\n"));
        assert!(text.contains("export function login"));
        assert_eq!(login.signature(), "export function login(): void {}");
    }
}
