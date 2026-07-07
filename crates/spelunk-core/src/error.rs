use std::path::PathBuf;

/// All fallible spelunk-core operations return this error.
///
/// Rust note: `thiserror` derives `std::error::Error` + `Display` from the
/// `#[error(...)]` attributes. `#[from]` additionally generates a `From` impl,
/// which is what lets `?` convert an `ignore::Error` into `Error::Walk`
/// automatically.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("error while walking files")]
    Walk(#[from] ignore::Error),

    #[error("invalid chunk query for {language}")]
    Query {
        language: &'static str,
        #[source]
        source: tree_sitter::QueryError,
    },

    #[error("tree-sitter rejected the {language} grammar")]
    Grammar {
        language: &'static str,
        #[source]
        source: tree_sitter::LanguageError,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
