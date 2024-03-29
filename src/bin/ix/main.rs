mod config;
mod searcher;
mod tui;

use crate::config::DatabaseConfig;
use indexa::{database::DatabaseBuilder, query::MatchPathMode};

use anyhow::{anyhow, Error, Result};
use dialoguer::Confirm;
use rayon::ThreadPoolBuilder;
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
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
        create_database(&config.database)?;
        return Ok(());
    }

    if !db_location.exists() {
        let yes = Confirm::new()
            .with_prompt("Database is not created yet. Create it now?")
            .default(true)
            .interact()?;
        if yes {
            create_database(&config.database)?;
        } else {
            return Ok(());
        }
    }

    tui::run(&config)?;

    Ok(())
}

fn create_database(db_config: &DatabaseConfig) -> Result<()> {
    let mut builder = DatabaseBuilder::new();
    builder.ignore_hidden(db_config.ignore_hidden);
    for dir in &db_config.dirs {
        builder.add_dir(&dir);
    }
    for kind in &db_config.index {
        builder.index(*kind);
    }
    for kind in &db_config.fast_sort {
        builder.fast_sort(*kind);
    }

    eprintln!("Indexing");
    let database = builder.build()?;
    eprintln!("Indexed {} files/directories", database.num_entries());

    eprintln!("Writing");

    let location = db_config.location.as_ref().unwrap();
    let create = !location.exists();

    if let Some(parent) = location.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut writer = BufWriter::new(File::create(&location)?);
    bincode::serialize_into(&mut writer, &database)?;
    writer.flush()?;

    if create {
        eprintln!("Created a database at {}", location.display());
    } else {
        eprintln!("Updated the database");
    }

    Ok(())
}
