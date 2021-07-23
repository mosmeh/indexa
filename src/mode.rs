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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag() {
        assert!(0b11.has_flag(1));
        assert!(0b11.has_flag(0));
        assert!(0b11.has_flag(0b10));
        assert!(!0b10.has_flag(1));
    }
}
