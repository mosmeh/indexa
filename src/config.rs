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
    pub index: Vec<IndexType>,
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
pub enum IndexType {
    Size,
    Mode,
    Created,
    Modified,
    Accessed,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UIConfig {
    pub sort_by: ColumnType,
    pub sort_order: SortOrder,
    pub columns: Vec<ColumnType>,
    pub basename_width_percentage: u16,
    pub human_readable_size: bool,
    pub datetime_format: String,
}

impl Default for UIConfig {
    fn default() -> Self {
        Self {
            sort_by: ColumnType::Basename,
            sort_order: SortOrder::Ascending,
            columns: vec![ColumnType::Basename, ColumnType::FullPath],
            basename_width_percentage: 30,
            human_readable_size: false,
            datetime_format: "%Y/%m/%d %T".to_string(),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
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

impl fmt::Display for ColumnType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ColumnType::FullPath => write!(f, "Path"),
            ColumnType::Basename => write!(f, "Basename"),
            ColumnType::Size => write!(f, "Size"),
            ColumnType::Mode => write!(f, "Mode"),
            ColumnType::Extension => write!(f, "Extension"),
            ColumnType::Created => write!(f, "Created"),
            ColumnType::Modified => write!(f, "Modified"),
            ColumnType::Accessed => write!(f, "Accessed"),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Ascending,
    Descending,
}
