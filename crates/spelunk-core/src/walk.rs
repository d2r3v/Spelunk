//! Repository walking: find the source files worth chunking.
//!
//! Built on the `ignore` crate (the engine behind ripgrep), so `.gitignore`,
//! `.ignore`, and global git excludes are respected for free.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::error::Result;
use crate::language::{self, LanguageConfig};

/// Files larger than this are skipped — generated bundles and vendored blobs,
/// not code anyone wants retrieved.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// A source file selected for chunking.
#[derive(Debug)]
pub struct SourceFile {
    /// Path relative to the walk root, `/`-separated on every platform so
    /// index contents and JSON output are portable.
    pub rel_path: String,
    /// Absolute-ish path usable for reading the file.
    pub abs_path: PathBuf,
    pub config: &'static LanguageConfig,
}

/// Walk `root` and return every supported source file, deterministically
/// sorted by relative path.
///
/// Rules:
/// - `.gitignore` / `.ignore` are honored even when `root` is not a git repo
///   (`require_git(false)`) — spelunk promises zero config, so ignore files
///   should always mean what they say.
/// - Hidden files and directories (dot-prefixed) are skipped, which also
///   covers `.git/`. `.spelunk/` is excluded explicitly as well so the index
///   never indexes itself, even if hidden-file handling changes.
/// - Only extensions registered in the language registry are returned.
/// - Files over [`MAX_FILE_BYTES`] are skipped.
pub fn walk_source_files(root: &Path) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();

    let walker = WalkBuilder::new(root)
        .require_git(false)
        .filter_entry(|entry| entry.file_name() != ".spelunk")
        .build();

    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let Some(config) = language::config_for_path(path) else {
            continue;
        };
        match entry.metadata() {
            Ok(meta) if meta.len() <= MAX_FILE_BYTES => {}
            // Oversized, or metadata unreadable (racing delete, permissions):
            // skip rather than fail the whole walk.
            _ => continue,
        }

        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel_path = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");

        files.push(SourceFile {
            rel_path,
            abs_path: path.to_path_buf(),
            config,
        });
    }

    // The walk order is filesystem-dependent; sort so output, tests, and
    // (later) index builds are deterministic across platforms.
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, contents: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn respects_gitignore_without_a_git_repo() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".gitignore", "dist/\n");
        write(root, "src/app.ts", "export function main() {}\n");
        write(root, "dist/bundle.ts", "function generated() {}\n");
        write(root, "notes.txt", "not source code\n");

        let files = walk_source_files(root).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["src/app.ts"]);
    }

    #[test]
    fn skips_hidden_spelunk_and_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "ok.ts", "const x = 1;\n");
        write(root, ".hidden/secret.ts", "const y = 2;\n");
        write(root, ".spelunk/cache.ts", "const z = 3;\n");
        write(root, "big.ts", &"// pad\n".repeat(200_000)); // ~1.4 MiB

        let files = walk_source_files(root).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["ok.ts"]);
    }

    #[test]
    fn results_are_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "b/z.ts", "const z = 1;\n");
        write(root, "a.ts", "const a = 1;\n");
        write(root, "b/a.tsx", "const t = 1;\n");

        let files = walk_source_files(root).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["a.ts", "b/a.tsx", "b/z.ts"]);
    }
}
