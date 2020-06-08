mod config;
mod tui;
mod worker;

use anyhow::Result;
use config::{Config, IndexKind};
use indexa::DatabaseBuilder;
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
    #[structopt(short = "s", long)]
    case_sensitive: bool,

    #[structopt(short = "p", long)]
    in_path: bool,

    #[structopt(short, long)]
    regex: bool,

    #[structopt(short, long)]
    update: bool,

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

        let mut db_builder = DatabaseBuilder::new(&config.database.dir);
        for kind in &config.database.index {
            match kind {
                IndexKind::Size => db_builder.size(true),
                IndexKind::Created => db_builder.created(true),
                IndexKind::Modified => db_builder.modified(true),
                IndexKind::Accessed => db_builder.accessed(true),
                IndexKind::Mode => db_builder.mode(true),
            };
        }
        let database = db_builder.build()?;

        let mut writer = BufWriter::new(File::create(&config.database.location)?);
        bincode::serialize_into(&mut writer, &database)?;
        writer.flush()?;
    }

    if !opt.update {
        tui::run(&config)?;
    }

    Ok(())
}
