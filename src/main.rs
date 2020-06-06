mod tui;

use anyhow::{anyhow, Result};
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
    #[structopt(short, long)]
    case_sensitive: bool,

    #[structopt(short = "p", long)]
    in_path: bool,

    #[structopt(short, long)]
    regex: bool,

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
    let mut opt = Opt::from_args();
    opt.location = Some(
        opt.location
            .or_else(dirs::home_dir)
            .ok_or_else(|| anyhow!("Cannot determine root directory to index"))?,
    );
    opt.threads.get_or_insert_with(num_cpus::get);

    tui::launch(&opt)?;

    Ok(())
}
