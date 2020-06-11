mod config;
mod tui;
mod worker;

use indexa::database::DatabaseBuilder;

use anyhow::Result;
use config::{Config, IndexKind};
use rayon::ThreadPoolBuilder;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(
    name = "indexa",
    author = env!("CARGO_PKG_AUTHORS"),
    rename_all = "kebab-case",
    setting(clap::AppSettings::ColoredHelp),
    setting(clap::AppSettings::DeriveDisplayOrder)
)]
pub struct Opt {
    /// Search case sensitively.
    #[structopt(short = "s", long)]
    case_sensitive: bool,

    /// Search in path.
    #[structopt(short = "p", long)]
    match_path: bool,

    /// Search in path when query contains path separators.
    #[structopt(long)]
    auto_match_path: bool,

    /// Enable regex.
    #[structopt(short, long)]
    regex: bool,

    /// Update database and exit.
    #[structopt(short, long)]
    update: bool,

    /// Number of threads to use.
    ///
    /// Defaults to the number of available CPUs - 1.
    #[structopt(short, long)]
    threads: Option<usize>,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    let mut config: Config = toml::from_str(&fs::read_to_string("config.toml")?)?;
    config.flags.merge_opt(&opt);

    ThreadPoolBuilder::new()
        .num_threads(config.flags.threads)
        .build_global()?;

    if opt.update || !config.database.location.exists() {
        if config.database.location.exists() {
            println!("Updating database");
        } else {
            println!("Creating database");
        }

        let mut db_builder = DatabaseBuilder::new();

        for dir in &config.database.dirs {
            db_builder.add_dir(&dir);
        }

        for kind in &config.database.index {
            match kind {
                IndexKind::Size => db_builder.size(true),
                IndexKind::Created => db_builder.created(true),
                IndexKind::Modified => db_builder.modified(true),
                IndexKind::Accessed => db_builder.accessed(true),
                IndexKind::Mode => db_builder.mode(true),
            };
        }

        let database = db_builder
            .ignore_hidden(config.database.ignore_hidden)
            .build()?;

        let mut writer = BufWriter::new(File::create(&config.database.location)?);
        bincode::serialize_into(&mut writer, &database)?;
        writer.flush()?;
    }

    if !opt.update {
        tui::run(&config)?;
    }

    Ok(())
}
