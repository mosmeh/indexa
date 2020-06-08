use super::Mode;
use std::fmt;
use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;

impl From<&Metadata> for Mode {
    fn from(metadata: &Metadata) -> Self {
        Self(metadata.mode())
    }
}

impl Mode {
    #[allow(dead_code)]
    pub fn display_octal(&self) -> DisplayOctal {
        DisplayOctal(self.0)
    }

    #[allow(dead_code)]
    pub fn display_symbol(&self) -> DisplaySymbol {
        DisplaySymbol(self.0)
    }
}

pub struct DisplayOctal(u32);

impl fmt::Display for DisplayOctal {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unimplemented!()
    }
}

pub struct DisplaySymbol(u32);

impl fmt::Display for DisplaySymbol {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        unimplemented!()
    }
}
