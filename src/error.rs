use std::io;
use std::time::SystemTimeError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    SystemTime(#[from] SystemTimeError),
    #[error(transparent)]
    RegexBuild(#[from] regex::Error),
    #[error("Invalid UTF-8 sequence")]
    Utf8,
    #[error("Invalid filename")]
    Filename,
    #[error("Search aborted")]
    SearchAbort,
}
