use super::{HasFlag, Mode};
use std::{
    fmt::{self, Write},
    fs::Metadata,
    os::unix::fs::MetadataExt,
};

const S_IFMT: u32 = 0xf000;
const S_IFIFO: u32 = 0x1000;
const S_IFCHR: u32 = 0x2000;
const S_IFDIR: u32 = 0x4000;
const S_IFBLK: u32 = 0x6000;
const S_IFREG: u32 = 0x8000;
const S_IFLNK: u32 = 0xa000;
const S_IFSOCK: u32 = 0xc000;

const S_ISUID: u32 = 0o4000;
const S_ISGID: u32 = 0o2000;
const S_ISVTX: u32 = 0o1000;

const S_IRUSR: u32 = 0o0400;
const S_IWUSR: u32 = 0o0200;
const S_IXUSR: u32 = 0o0100;

const S_IRGRP: u32 = 0o0040;
const S_IWGRP: u32 = 0o0020;
const S_IXGRP: u32 = 0o0010;

const S_IROTH: u32 = 0o0004;
const S_IWOTH: u32 = 0o0002;
const S_IXOTH: u32 = 0o0001;

impl From<&Metadata> for Mode {
    fn from(metadata: &Metadata) -> Self {
        Self(metadata.mode())
    }
}

impl Mode {
    pub fn display_octal(&self) -> DisplayOctal {
        DisplayOctal(self.0)
    }

    pub fn display_symbolic(&self) -> DisplaySymbolic {
        DisplaySymbolic(self.0)
    }
}

pub struct DisplayOctal(u32);

impl fmt::Display for DisplayOctal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04o}", self.0 & 0o7777)
    }
}

pub struct DisplaySymbolic(u32);

impl fmt::Display for DisplaySymbolic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 & S_IFMT {
            S_IFIFO => f.write_char('p')?,
            S_IFCHR => f.write_char('c')?,
            S_IFDIR => f.write_char('d')?,
            S_IFBLK => f.write_char('b')?,
            S_IFREG => f.write_char('-')?,
            S_IFLNK => f.write_char('l')?,
            S_IFSOCK => f.write_char('s')?,
            _ => f.write_char('-')?,
        };

        f.write_char(if self.0.has_flag(S_IRUSR) { 'r' } else { '-' })?;
        f.write_char(if self.0.has_flag(S_IWUSR) { 'w' } else { '-' })?;
        f.write_char(match (self.0.has_flag(S_IXUSR), self.0.has_flag(S_ISUID)) {
            (false, false) => '-',
            (true, false) => 'x',
            (false, true) => 'S',
            (true, true) => 's',
        })?;

        f.write_char(if self.0.has_flag(S_IRGRP) { 'r' } else { '-' })?;
        f.write_char(if self.0.has_flag(S_IWGRP) { 'w' } else { '-' })?;
        f.write_char(match (self.0.has_flag(S_IXGRP), self.0.has_flag(S_ISGID)) {
            (false, false) => '-',
            (true, false) => 'x',
            (false, true) => 'S',
            (true, true) => 's',
        })?;

        f.write_char(if self.0.has_flag(S_IROTH) { 'r' } else { '-' })?;
        f.write_char(if self.0.has_flag(S_IWOTH) { 'w' } else { '-' })?;
        f.write_char(match (self.0.has_flag(S_IXOTH), self.0.has_flag(S_ISVTX)) {
            (false, false) => '-',
            (true, false) => 'x',
            (false, true) => 'T',
            (true, true) => 't',
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(mode: u32, octal: &str, symbolic: &str) {
        let mode = Mode::from(mode);
        assert_eq!(format!("{}", mode.display_octal()), octal);
        assert_eq!(format!("{}", mode.display_symbolic()), symbolic);
    }

    #[test]
    fn check_both() {
        check(0o0555, "0555", "-r-xr-xr-x");
        check(0o0600, "0600", "-rw-------");
        check(0o0644, "0644", "-rw-r--r--");
        check(0o0664, "0664", "-rw-rw-r--");
        check(0o0755, "0755", "-rwxr-xr-x");
        check(0o1600, "1600", "-rw------T");
        check(0o1777, "1777", "-rwxrwxrwt");
        check(0o2745, "2745", "-rwxr-Sr-x");
        check(0o2755, "2755", "-rwxr-sr-x");
        check(0o4455, "4455", "-r-Sr-xr-x");
        check(0o4555, "4555", "-r-sr-xr-x");
        check(0o020444, "0444", "cr--r--r--");
        check(0o040700, "0700", "drwx------");
        check(0o060640, "0640", "brw-r-----");
        check(0o100555, "0555", "-r-xr-xr-x");
        check(0o100600, "0600", "-rw-------");
        check(0o100664, "0664", "-rw-rw-r--");
        check(0o120755, "0755", "lrwxr-xr-x");
    }
}
