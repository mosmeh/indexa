use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Regex(#[from] regex::Error),
    #[error(transparent)]
    RegexSyntax(#[from] regex_syntax::Error),
    #[error("{0}")]
    InvalidOption(String),
    #[error("Encountered non-UTF-8 path")]
    NonUtf8Path,
    #[error("Search aborted")]
    SearchAbort,
}
