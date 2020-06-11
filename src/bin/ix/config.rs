use crate::Opt;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub flags: FlagConfig,
    pub database: DatabaseConfig,
    pub ui: UIConfig,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FlagConfig {
    pub case_sensitive: bool,
    pub match_path: bool,
    pub auto_match_path: bool,
    pub regex: bool,
    pub threads: usize,
}

impl Default for FlagConfig {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            match_path: false,
            auto_match_path: false,
            regex: false,
            threads: (num_cpus::get() - 1).max(1),
        }
    }
}

impl FlagConfig {
    pub fn merge_opt(&mut self, opt: &Opt) {
        self.case_sensitive |= opt.case_sensitive;
        self.match_path |= opt.match_path;
        self.auto_match_path |= opt.auto_match_path;
        self.regex |= opt.regex;

        if let Some(threads) = opt.threads {
            self.threads = threads.min(num_cpus::get() - 1).max(1);
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub location: PathBuf,
    pub index: Vec<IndexKind>,
    pub dirs: Vec<PathBuf>,
    pub ignore_hidden: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            location: PathBuf::from("database"),
            index: Vec::new(),
            dirs: vec![dirs::home_dir().unwrap()],
            ignore_hidden: false,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndexKind {
    Size,
    Mode,
    Created,
    Modified,
    Accessed,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UIConfig {
    pub sort_by: ColumnKind,
    pub sort_order: SortOrder,
    pub human_readable_size: bool,
    pub datetime_format: String,
    pub columns: Vec<Column>,

    pub unix: UIConfigUnix,
    pub windows: UIConfigWindows,
}

impl Default for UIConfig {
    fn default() -> Self {
        Self {
            sort_by: ColumnKind::Basename,
            sort_order: SortOrder::Ascending,
            human_readable_size: false,
            datetime_format: "%Y/%m/%d %T".to_string(),
            columns: vec![
                Column {
                    kind: ColumnKind::Basename,
                    width: None,
                },
                Column {
                    kind: ColumnKind::FullPath,
                    width: None,
                },
            ],
            unix: Default::default(),
            windows: Default::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Column {
    pub kind: ColumnKind,
    pub width: Option<u16>,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnKind {
    Basename,
    #[serde(rename = "path")]
    FullPath,
    Extension,
    Size,
    Mode,
    Created,
    Modified,
    Accessed,
}

impl fmt::Display for ColumnKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColumnKind::FullPath => f.write_str("Path"),
            ColumnKind::Basename => f.write_str("Basename"),
            ColumnKind::Size => f.write_str("Size"),
            ColumnKind::Mode => f.write_str("Mode"),
            ColumnKind::Extension => f.write_str("Extension"),
            ColumnKind::Created => f.write_str("Created"),
            ColumnKind::Modified => f.write_str("Modified"),
            ColumnKind::Accessed => f.write_str("Accessed"),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UIConfigUnix {
    pub mode_format: ModeFormatUnix,
}

impl Default for UIConfigUnix {
    fn default() -> Self {
        Self {
            mode_format: ModeFormatUnix::Symbolic,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeFormatUnix {
    Octal,
    Symbolic,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UIConfigWindows {
    pub mode_format: ModeFormatWindows,
}

impl Default for UIConfigWindows {
    fn default() -> Self {
        Self {
            mode_format: ModeFormatWindows::Traditional,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeFormatWindows {
    Traditional,
    PowerShell,
}
