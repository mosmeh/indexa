mod tui;
mod worker;

use anyhow::{anyhow, Result};
use indexa::Database;
use rayon::ThreadPoolBuilder;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
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
    human_readable: bool,

    #[structopt(short, long)]
    update: bool,

    #[structopt(short, long, default_value = "database")]
    database: PathBuf,

    #[structopt(short, long)]
    location: Option<PathBuf>,

    #[structopt(short, long)]
    threads: Option<usize>,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    let location = opt
        .location
        .clone()
        .or_else(dirs::home_dir)
        .ok_or_else(|| anyhow!("Cannot determine root directory to index"))?;
    let threads = opt.threads.unwrap_or_else(|| num_cpus::get() - 1);

    ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()?;

    if opt.update || !opt.database.exists() {
        if opt.database.exists() {
            println!("Updating database");
        } else {
            println!("Creating database");
        }

        let database = Database::new(&location)?;
        let mut writer = BufWriter::new(File::create(&opt.database)?);
        bincode::serialize_into(&mut writer, &database)?;
        writer.flush()?;
    }

    if !opt.update {
        tui::run(&opt)?;
    }

    Ok(())
}
