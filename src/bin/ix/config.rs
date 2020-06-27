use crate::Opt;

use indexa::database::StatusKind;
use indexa::query::SortOrder;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub flags: FlagConfig,
    pub database: DatabaseConfig,
    pub ui: UIConfig,
}

#[derive(Debug, Serialize, PartialEq, Deserialize)]
#[serde(default)]
pub struct FlagConfig {
    pub query: Option<String>,
    pub case_sensitive: bool,
    pub match_path: bool,
    pub auto_match_path: bool,
    pub regex: bool,
    pub threads: usize,
}

impl Default for FlagConfig {
    fn default() -> Self {
        Self {
            query: None,
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
        if let Some(query) = &opt.query {
            self.query = Some(query.clone());
        }

        self.case_sensitive |= opt.case_sensitive;
        self.match_path |= opt.match_path;
        self.auto_match_path |= opt.auto_match_path;
        self.regex |= opt.regex;

        if let Some(threads) = opt.threads {
            self.threads = threads.min(num_cpus::get() - 1).max(1);
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub location: Option<PathBuf>,
    pub index: Vec<StatusKind>,
    pub fast_sort: Vec<StatusKind>,
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
            PathBuf::from(r"C:\")
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
            fast_sort: Vec::new(),
            dirs,
            ignore_hidden: false,
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UIConfig {
    pub sort_by: StatusKind,
    pub sort_order: SortOrder,
    pub sort_dirs_before_files: bool,
    pub human_readable_size: bool,
    pub datetime_format: String,
    pub columns: Vec<Column>,

    pub unix: UIConfigUnix,
    pub windows: UIConfigWindows,
}

impl Default for UIConfig {
    fn default() -> Self {
        Self {
            sort_by: StatusKind::Basename,
            sort_order: SortOrder::Ascending,
            sort_dirs_before_files: false,
            human_readable_size: true,
            datetime_format: "%Y/%m/%d %T".to_string(),
            columns: vec![
                Column {
                    status: StatusKind::Basename,
                    width: None,
                },
                Column {
                    status: StatusKind::Size,
                    width: Some(10),
                },
                Column {
                    status: StatusKind::Modified,
                    width: Some(20),
                },
                Column {
                    status: StatusKind::FullPath,
                    width: None,
                },
            ],
            unix: Default::default(),
            windows: Default::default(),
        }
    }
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct Column {
    pub status: StatusKind,
    pub width: Option<u16>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeFormatUnix {
    Octal,
    Symbolic,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeFormatWindows {
    Traditional,
    PowerShell,
}

const DEFAULT_CONFIG: &str = include_str!("../../../config/default.toml");

pub fn read_or_create_config<P>(config_path: Option<P>) -> Result<Config>
where
    P: AsRef<Path>,
{
    const CONFIG_LOCATION_ERROR_MSG: &str = "Could not determine the location of config file. \
    Please provide the location of config file with -C/--config option.";

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

        let mut writer = BufWriter::new(File::create(&path)?);
        writer.write_all(DEFAULT_CONFIG.as_bytes())?;
        writer.flush()?;

        eprintln!("Created a default configuration file at {}", path.display());

        Ok(toml::from_str(DEFAULT_CONFIG)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn create_and_read() {
        let config_file = NamedTempFile::new().unwrap();
        let config_path = config_file.path();

        let default_config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        let created_config = read_or_create_config(Some(config_path)).unwrap();
        assert_eq!(default_config, created_config);

        let read_config = read_or_create_config(Some(config_path)).unwrap();
        assert_eq!(created_config, read_config);
    }

    #[test]
    fn empty() {
        let empty_file = NamedTempFile::new().unwrap();

        let default_config: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        let config = read_or_create_config(Some(empty_file.path())).unwrap();
        assert_eq!(default_config, config);
    }

    #[test]
    #[should_panic(expected = "Invalid config file")]
    fn invalid() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "xxx").unwrap();

        read_or_create_config(Some(file.path())).unwrap();
    }
}
