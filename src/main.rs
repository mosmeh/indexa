use anyhow::{anyhow, Result};
use clap::{clap_app, value_t};
use crossbeam::channel;
use indexa::Database;
use rayon::prelude::*;
use rayon::ThreadPoolBuilder;
use regex::RegexBuilder;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;

fn main() -> Result<()> {
    let matches = clap_app!(ix =>
        (version: env!("CARGO_PKG_VERSION"))
        (author: env!("CARGO_PKG_AUTHORS"))
        (about: env!("CARGO_PKG_DESCRIPTION"))
        (@arg PATTERN: * +takes_value)
        (@arg ("case-sensitive"): -s --("case-sensitive"))
        (@arg regex: -r --regex)
        (@arg database: -d --database +takes_value)
        (@arg update: -u --update)
        (@arg location: -l --location +takes_value)
        (@arg threads: -t --threads +takes_value)
    )
    .get_matches();

    let pattern = value_t!(matches, "PATTERN", String)?;
    let db_path =
        value_t!(matches, "database", PathBuf).unwrap_or_else(|_| PathBuf::from("database"));
    let location = value_t!(matches, "location", PathBuf).or_else(|_| {
        dirs::home_dir().ok_or_else(|| anyhow!("Cannot determine root directory to index"))
    })?;
    let threads = value_t!(matches, "threads", usize).unwrap_or_else(|_| num_cpus::get());

    let pool = ThreadPoolBuilder::new().num_threads(threads).build()?;

    let database = if matches.is_present("update") || !db_path.exists() {
        println!("Updating database");
        let database = pool.install(|| Database::new(location))?;
        let mut writer = BufWriter::new(File::create(&db_path)?);
        bincode::serialize_into(&mut writer, &database)?;
        writer.flush()?;
        database
    } else {
        println!("Loading database");
        let reader = BufReader::new(File::open(&db_path)?);
        bincode::deserialize_from(reader)?
    };
    println!("Finished");

    let pattern = if matches.is_present("regex") {
        pattern
    } else {
        regex::escape(&pattern)
    };
    let pattern = RegexBuilder::new(&pattern)
        .case_insensitive(!matches.is_present("case-sensitive"))
        .build()?;

    let hits = pool.install(|| {
        let (tx, rx) = channel::unbounded();
        let _ = database.search(&pattern, tx);

        let mut hits = rx.iter().collect::<Vec<_>>();
        hits.as_parallel_slice_mut()
            .par_sort_unstable_by_key(|hit| hit.status.mtime);
        hits
    });

    println!("{}", hits.len());
    for x in hits.iter().rev().take(10) {
        println!("{}", x.path.display());
    }

    Ok(())
}
