#[cfg(unix)]
pub mod unix;

#[cfg(windows)]
pub mod windows;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Mode(u32);

impl Default for Mode {
    fn default() -> Self {
        Self(0)
    }
}

impl From<u32> for Mode {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl Mode {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Default::default()
    }
}

trait HasFlag: Copy {
    fn has_flag(&self, other: Self) -> bool;
}

impl HasFlag for u32 {
    fn has_flag(&self, flag: Self) -> bool {
        self & flag == flag
    }
}
