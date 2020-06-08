mod database;
mod error;
pub mod mode;

pub use error::Error;
pub type Result<T> = std::result::Result<T, Error>;

pub use database::{Database, DatabaseBuilder, Entry, EntryId};
