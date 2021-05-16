use super::{HasFlag, Mode};
use std::{
    fmt::{self, Write},
    fs::Metadata,
    os::windows::fs::MetadataExt,
};

const FILE_ATTRIBUTE_READONLY: u32 = 0x00000001;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x00000002;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x00000004;
const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x00000010;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x00000020;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x00000400;

const ATTRIBUTE_CHARS: [char; 21] = [
    'R', 'H', 'S', 'V', 'D', 'A', 'X', 'N', 'T', 'P', 'L', 'C', 'O', 'I', 'E', 'V', '\0', 'X',
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

#[cfg(test)]
mod tests {
    use super::*;

    fn check(mode: u32, powershell: &str, traditional: &str) {
        let mode = Mode::from(mode);
        assert_eq!(format!("{}", mode.display_powershell()), powershell);
        assert_eq!(format!("{}", mode.display_traditional()), traditional);
    }

    #[test]
    fn check_both() {
        check(0x0000, "-----", "");
        check(0x0010, "d----", "D");
        check(0x0020, "-a---", "A");
        check(0x0021, "-ar--", "RA");
        check(0x0023, "-arh-", "RHA");
        check(0x0024, "-a--s", "SA");
        check(0x0027, "-arhs", "RHSA");
        check(0x0122, "-a-h-", "HAT");
        check(0x0220, "-a---", "AP");
        check(0x0410, "l----", "DL");
        check(0x0420, "la---", "AL");
        check(0x0820, "-a---", "AC");
        check(0x1010, "d----", "DO");
        check(0x1224, "-a--s", "SAPO");
        check(0x1326, "-a-hs", "HSATPO");
        check(0x2004, "----s", "SI");
        check(0x2020, "-a---", "AI");
        check(0x2024, "-a--s", "SAI");
        check(0x2026, "-a-hs", "HSAI");
        check(0x2920, "-a---", "ATCI");
        check(0x200000 - 1, "larhs", "RHSVDAXNTPLCOIEVXPU");
    }
}
