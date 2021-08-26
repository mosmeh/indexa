pub use camino;
pub use enum_map;
pub use strum;

pub mod database;
mod error;
pub mod mode;
pub mod query;

pub use error::Error;
pub type Result<T> = std::result::Result<T, Error>;
