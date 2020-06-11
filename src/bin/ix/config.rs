use crate::Opt;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fmt;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

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
    pub location: Option<PathBuf>,
    pub index: Vec<IndexKind>,
    pub dirs: Vec<PathBuf>,
    pub ignore_hidden: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        let location = dirs::data_dir().map(|data_dir| {
            let mut path = data_dir;
            path.push(env!("CARGO_PKG_NAME"));
            path.push("database.db");
            path
        });

        let default_root = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        let dirs = if default_root.exists() {
            vec![default_root]
        } else {
            Vec::new()
        };

        Self {
            location,
            index: Vec::new(),
            dirs,
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

const DEFAULT_CONFIG: &str = include_str!("../../../config/default.toml");

const CONFIG_LOCATION_ERROR_MSG: &str = "Could not determine the location of config file. \
    Please provide the location of config file with -C/--config option.";

pub fn read_or_create_config<P>(config_path: Option<P>) -> Result<Config>
where
    P: AsRef<Path>,
{
    let path = if let Some(path) = config_path.as_ref() {
        Cow::Borrowed(path.as_ref())
    } else if cfg!(windows) {
        let config_dir = dirs::config_dir().ok_or_else(|| anyhow!(CONFIG_LOCATION_ERROR_MSG))?;
        let mut path = config_dir;
        path.push(env!("CARGO_PKG_NAME"));
        path.push("config.toml");
        Cow::Owned(path)
    } else {
        let home_dir = dirs::home_dir().ok_or_else(|| anyhow!(CONFIG_LOCATION_ERROR_MSG))?;
        let mut path = home_dir;
        path.push(".config");
        path.push(env!("CARGO_PKG_NAME"));
        path.push("config.toml");
        Cow::Owned(path)
    };

    if let Ok(config_string) = fs::read_to_string(&path) {
        Ok(toml::from_str(config_string.as_str())
            .context("Invalid config file. Please edit the config file and try again.")?)
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut writer = BufWriter::new(File::create(path)?);
        writer.write_all(DEFAULT_CONFIG.as_bytes())?;
        writer.flush()?;

        Ok(toml::from_str(DEFAULT_CONFIG)?)
    }
}
