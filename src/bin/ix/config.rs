use crate::Opt;

use indexa::database::StatusKind;
use indexa::query::{CaseSensitivity, MatchPathMode, SortOrder};

use anyhow::{anyhow, Context, Result};
use itertools::Itertools;
use serde::{Deserialize, Deserializer};
use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use tui::style::Color;

#[derive(Debug, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub flags: FlagConfig,
    pub database: DatabaseConfig,
    pub ui: UIConfig,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FlagConfig {
    pub query: Option<String>,
    pub case_sensitive: bool,
    pub ignore_case: bool,
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
            ignore_case: false,
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

        // HACK: case_sensitive takes precedence over ignore_case in config file
        // TODO: make them mutually exclusive as in CLI flags
        if self.case_sensitive && self.ignore_case {
            self.case_sensitive = true;
            self.ignore_case = false;
        }

        if opt.case_sensitive || opt.ignore_case {
            self.case_sensitive = opt.case_sensitive;
            self.ignore_case = opt.ignore_case;
        }

        self.match_path |= opt.match_path;
        self.auto_match_path |= opt.auto_match_path;
        self.regex |= opt.regex;

        if let Some(threads) = opt.threads {
            self.threads = threads.min(num_cpus::get() - 1).max(1);
        }
    }

    pub fn match_path_mode(&self) -> MatchPathMode {
        if self.match_path {
            MatchPathMode::Always
        } else if self.auto_match_path {
            MatchPathMode::Auto
        } else {
            MatchPathMode::Never
        }
    }

    pub fn case_sensitivity(&self) -> CaseSensitivity {
        if self.case_sensitive {
            CaseSensitivity::Sensitive
        } else if self.ignore_case {
            CaseSensitivity::Insensitive
        } else {
            CaseSensitivity::Smart
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UIConfig {
    pub sort_by: StatusKind,
    pub sort_order: SortOrder,
    pub sort_dirs_before_files: bool,
    pub human_readable_size: bool,
    pub datetime_format: String,
    pub column_spacing: u16,
    pub columns: Vec<Column>,
    pub unix: UIConfigUnix,
    pub windows: UIConfigWindows,
    pub colors: ColorConfig,
}

impl Default for UIConfig {
    fn default() -> Self {
        Self {
            sort_by: StatusKind::Basename,
            sort_order: SortOrder::Ascending,
            sort_dirs_before_files: false,
            human_readable_size: true,
            datetime_format: "%Y-%m-%d %R".to_string(),
            column_spacing: 2,
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
                    width: Some(16),
                },
                Column {
                    status: StatusKind::FullPath,
                    width: None,
                },
            ],
            unix: Default::default(),
            windows: Default::default(),
            colors: Default::default(),
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
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

#[derive(Debug, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ColorConfig {
    #[serde(deserialize_with = "deserialize_color")]
    pub selected_fg: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub selected_bg: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub matched_fg: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub matched_bg: Color,
    #[serde(deserialize_with = "deserialize_color")]
    pub prompt: Color,
}

impl Default for ColorConfig {
    fn default() -> Self {
        Self {
            selected_fg: Color::LightBlue,
            selected_bg: Color::Reset,
            matched_fg: Color::Black,
            matched_bg: Color::LightBlue,
            prompt: Color::LightBlue,
        }
    }
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct Column {
    pub status: StatusKind,
    pub width: Option<u16>,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModeFormatUnix {
    Octal,
    Symbolic,
}

#[derive(Debug, PartialEq, Deserialize)]
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

        Ok(Default::default())
    }
}

fn deserialize_color<'de, D>(deserializer: D) -> Result<Color, D::Error>
where
    D: Deserializer<'de>,
{
    let string = String::deserialize(deserializer)?;

    match string.trim().to_lowercase().as_str() {
        "reset" => Ok(Color::Reset),
        "black" => Ok(Color::Black),
        "red" => Ok(Color::Red),
        "green" => Ok(Color::Green),
        "yellow" => Ok(Color::Yellow),
        "blue" => Ok(Color::Blue),
        "magenta" => Ok(Color::Magenta),
        "cyan" => Ok(Color::Cyan),
        "gray" => Ok(Color::Gray),
        "darkgray" => Ok(Color::DarkGray),
        "lightred" => Ok(Color::LightRed),
        "lightgreen" => Ok(Color::LightGreen),
        "lightyellow" => Ok(Color::LightYellow),
        "lightblue" => Ok(Color::LightBlue),
        "lightmagenta" => Ok(Color::LightMagenta),
        "lightcyan" => Ok(Color::LightCyan),
        "white" => Ok(Color::White),
        string => {
            let components: Result<Vec<_>, _> = match string {
                hex if hex.starts_with('#') && hex.len() == 4 => hex
                    .chars()
                    .skip(1)
                    .map(|x| u8::from_str_radix(&format!("{}{}", x, x), 16))
                    .collect(),
                hex if hex.starts_with('#') && hex.len() == 7 => hex
                    .chars()
                    .skip(1)
                    .tuples()
                    .map(|(a, b)| u8::from_str_radix(&format!("{}{}", a, b), 16))
                    .collect(),
                rgb => rgb.split(',').map(|c| c.trim().parse::<u8>()).collect(),
            };
            if let Ok(components) = components {
                if let [r, g, b] = *components {
                    return Ok(Color::Rgb(r, g, b));
                }
            }
            Err(serde::de::Error::custom("Invalid color"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn create_and_read_config() {
        let tmpdir = tempfile::tempdir().unwrap();
        let nonexistent_file = tmpdir.path().join("config.toml");
        let created_config = read_or_create_config(Some(&nonexistent_file)).unwrap();

        let created_file = nonexistent_file;
        let read_config = read_or_create_config(Some(created_file)).unwrap();

        assert_eq!(created_config, read_config);
    }

    #[test]
    fn default_config_is_consistent() {
        let from_str: Config = toml::from_str(DEFAULT_CONFIG).unwrap();
        let from_default_trait = Config::default();

        assert_eq!(from_str, from_default_trait);

        let tmpdir = tempfile::tempdir().unwrap();
        let nonexistent_file = tmpdir.path().join("config.toml");
        let created = read_or_create_config(Some(nonexistent_file)).unwrap();

        assert_eq!(from_str, created);

        let empty_file = NamedTempFile::new().unwrap();
        let written = read_or_create_config(Some(empty_file.path())).unwrap();

        assert_eq!(from_str, written);
    }

    #[test]
    #[should_panic(expected = "Invalid config file")]
    fn invalid_config() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "xxx").unwrap();

        read_or_create_config(Some(file.path())).unwrap();
    }

    #[test]
    fn color() {
        use serde::de::IntoDeserializer;
        use tui::style::Color;

        type Deserializer<'a> = serde::de::value::StrDeserializer<'a, serde::de::value::Error>;

        let s: Deserializer = "blue".into_deserializer();
        assert_eq!(deserialize_color(s), Ok(Color::Blue));

        let s: Deserializer = "\t Red \r\n".into_deserializer();
        assert_eq!(deserialize_color(s), Ok(Color::Red));

        let s: Deserializer = "66, 135, 245".into_deserializer();
        assert_eq!(deserialize_color(s), Ok(Color::Rgb(66, 135, 245)));

        let s: Deserializer = "#E43".into_deserializer();
        assert_eq!(deserialize_color(s), Ok(Color::Rgb(238, 68, 51)));

        let s: Deserializer = "#fcba03".into_deserializer();
        assert_eq!(deserialize_color(s), Ok(Color::Rgb(252, 186, 3)));
    }
}
