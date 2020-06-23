pub mod database;
mod error;
pub mod mode;
pub mod query;

pub use error::Error;
pub type Result<T> = std::result::Result<T, Error>;
