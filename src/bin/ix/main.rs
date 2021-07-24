mod config;
mod tui;
mod worker;

use crate::config::Config;

use indexa::{database::DatabaseBuilder, query::MatchPathMode};

use anyhow::{anyhow, Error, Result};
use dialoguer::Confirm;
use rayon::ThreadPoolBuilder;
use std::{
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    str::FromStr,
};
use structopt::{clap::AppSettings, StructOpt};

#[derive(Debug, Clone, Copy)]
struct MatchPathOpt(MatchPathMode);

impl FromStr for MatchPathOpt {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let m = match s.to_lowercase().as_str() {
            "always" | "yes" => MatchPathMode::Always,
            "never" | "no" => MatchPathMode::Never,
            "auto" => MatchPathMode::Auto,
            _ => {
                return Err(anyhow!(format!(
                    "Invalid value '{}'. Valid values are 'always', 'never', or 'auto'.",
                    s
                )))
            }
        };
        Ok(Self(m))
    }
}

#[derive(Debug, StructOpt)]
#[structopt(
    name = "indexa",
    author = env!("CARGO_PKG_AUTHORS"),
    rename_all = "kebab-case",
    setting(AppSettings::ColoredHelp),
    setting(AppSettings::DeriveDisplayOrder),
    setting(AppSettings::AllArgsOverrideSelf)
)]
pub struct Opt {
    /// Initial query.
    #[structopt(short = "q", long)]
    query: Option<String>,

    /// Search case-sensitively.
    ///
    /// Defaults to smart case.
    #[structopt(short = "s", long, overrides_with_all = &["ignore-case", "case-sensitive"])]
    case_sensitive: bool,

    /// Search case-insensitively.
    ///
    /// Defaults to smart case.
    #[structopt(short = "i", long, overrides_with_all = &["case-sensitive", "ignore-case"])]
    ignore_case: bool,

    /// Match path.
    ///
    /// <when> can be 'always' (default if omitted), 'auto', or 'never'.
    /// With 'auto', it matches path only when query contains path separators.
    ///
    /// Defaults to 'never'.
    #[structopt(short = "p", long, name = "when")]
    match_path: Option<Option<MatchPathOpt>>,

    /// Enable regex.
    #[structopt(short, long)]
    regex: bool,

    /// Update database and exit.
    #[structopt(short, long)]
    update: bool,

    /// Number of threads to use.
    ///
    /// Defaults to the number of available CPUs minus 1.
    #[structopt(short, long)]
    threads: Option<usize>,

    /// Location of a config file.
    #[structopt(short = "C", long)]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    let mut config = config::read_or_create_config(opt.config.as_ref())?;
    config.flags.merge_opt(&opt);

    let db_location = if let Some(location) = &config.database.location {
        location
    } else {
        return Err(anyhow!(
            "Could not determine the location of the database file. Please edit the config file."
        ));
    };

    ThreadPoolBuilder::new()
        .num_threads(config.flags.threads)
        .build_global()?;

    if opt.update {
        create_database(db_location, &config)?;
        return Ok(());
    } else if !db_location.exists() {
        let yes = Confirm::new()
            .with_prompt("Database is not created yet. Create it now?")
            .interact_on(&dialoguer::console::Term::stderr())
            .unwrap_or(false);
        if yes {
            create_database(db_location, &config)?;
        } else {
            return Ok(());
        }
    }

    tui::run(&config)?;

    Ok(())
}

fn create_database<P: AsRef<Path>>(path: P, config: &Config) -> Result<()> {
    let create = !path.as_ref().exists();
    if create {
        eprintln!("Creating a database");
    } else {
        eprintln!("Updating the database");
    }

    let mut builder = DatabaseBuilder::new();
    for dir in &config.database.dirs {
        builder.add_dir(&dir);
    }
    for kind in &config.database.index {
        builder.index(*kind);
    }
    for kind in &config.database.fast_sort {
        builder.fast_sort(*kind);
    }

    let database = builder
        .ignore_hidden(config.database.ignore_hidden)
        .build()?;

    if let Some(parent) = path.as_ref().parent() {
        fs::create_dir_all(parent)?;
    }

    let mut writer = BufWriter::new(File::create(&path)?);
    bincode::serialize_into(&mut writer, &database)?;
    writer.flush()?;

    if create {
        eprintln!("Created a database at {}", path.as_ref().display());
    }

    Ok(())
}
