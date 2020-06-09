use super::{HasFlag, Mode};
use std::fmt::{self, Write};
use std::fs::Metadata;
use std::os::windows::fs::MetadataExt;

const FILE_ATTRIBUTE_READONLY: u32 = 0x00000001;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x00000002;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x00000004;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x00000010;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x00000020;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;

const ATTRIBUTE_CHARS: [char; 21] = [
    'R', 'H', 'S', '8', 'D', 'A', 'd', 'N', 'T', 's', 'L', 'C', 'O', 'I', 'E', 'V', '\0', 'X',
    '\0', 'P', 'U',
];

impl From<&Metadata> for Mode {
    fn from(metadata: &Metadata) -> Self {
        Self(metadata.file_attributes())
    }
}

impl Mode {
    pub fn is_hidden(&self) -> bool {
        self.0.has_flag(FILE_ATTRIBUTE_HIDDEN)
    }

    pub fn display_traditional(&self) -> DisplayTraditional {
        DisplayTraditional(self.0)
    }

    pub fn display_powershell(&self) -> DisplayPowerShell {
        DisplayPowerShell(self.0)
    }
}

pub struct DisplayTraditional(u32);

impl fmt::Display for DisplayTraditional {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, &c) in ATTRIBUTE_CHARS.iter().enumerate() {
            if c != '\0' && self.0.has_flag(1 << i) {
                f.write_char(c)?;
            }
        }
        Ok(())
    }
}

pub struct DisplayPowerShell(u32);

impl fmt::Display for DisplayPowerShell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char(if self.0.has_flag(FILE_ATTRIBUTE_REPARSE_POINT) {
            'l'
        } else if self.0.has_flag(FILE_ATTRIBUTE_DIRECTORY) {
            'd'
        } else {
            '-'
        })?;
        f.write_char(if self.0.has_flag(FILE_ATTRIBUTE_ARCHIVE) {
            'a'
        } else {
            '-'
        })?;
        f.write_char(if self.0.has_flag(FILE_ATTRIBUTE_READONLY) {
            'r'
        } else {
            '-'
        })?;
        f.write_char(if self.0.has_flag(FILE_ATTRIBUTE_HIDDEN) {
            'h'
        } else {
            '-'
        })?;
        f.write_char(if self.0.has_flag(FILE_ATTRIBUTE_SYSTEM) {
            's'
        } else {
            '-'
        })?;
        Ok(())
    }
}
