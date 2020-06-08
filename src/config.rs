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
    pub in_path: bool,
    pub regex: bool,
    pub threads: usize,
}

impl Default for FlagConfig {
    fn default() -> Self {
        Self {
            case_sensitive: false,
            in_path: false,
            regex: false,
            threads: (num_cpus::get() - 1).max(1),
        }
    }
}

impl FlagConfig {
    pub fn merge_opt(&mut self, opt: &Opt) {
        self.case_sensitive |= opt.case_sensitive;
        self.in_path |= opt.in_path;
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
    pub dir: PathBuf,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            location: PathBuf::from("database"),
            index: Vec::new(),
            dir: dirs::home_dir().unwrap(),
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
            ColumnKind::FullPath => write!(f, "Path"),
            ColumnKind::Basename => write!(f, "Basename"),
            ColumnKind::Size => write!(f, "Size"),
            ColumnKind::Mode => write!(f, "Mode"),
            ColumnKind::Extension => write!(f, "Extension"),
            ColumnKind::Created => write!(f, "Created"),
            ColumnKind::Modified => write!(f, "Modified"),
            ColumnKind::Accessed => write!(f, "Accessed"),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Ascending,
    Descending,
}
