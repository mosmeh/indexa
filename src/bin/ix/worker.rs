use crate::config::{ColumnKind, Config, SortOrder};

use indexa::database::{Database, Entry, EntryId};
use indexa::matcher::Matcher;

use anyhow::Result;
use crossbeam::channel::{self, Receiver, Sender};
use rayon::prelude::*;
use std::cmp;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{self, AtomicBool};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub struct Loader {
    thread: Option<JoinHandle<()>>,
}

impl Drop for Loader {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Loader {
    pub fn run<P>(db_path: P, tx: Sender<Result<Database>>) -> Result<Self>
    where
        P: 'static + AsRef<Path> + Send,
    {
        let thread = thread::spawn(move || {
            let _ = tx.send(load_database(db_path));
        });

        let loader = Self {
            thread: Some(thread),
        };

        Ok(loader)
    }
}

pub struct Searcher {
    thread: Option<JoinHandle<()>>,
    stop_tx: Sender<()>,
}

impl Drop for Searcher {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Searcher {
    pub fn run(
        config: &Config,
        database: Arc<Database>,
        rx: Receiver<Matcher>,
        tx: Sender<Vec<EntryId>>,
    ) -> Result<Self> {
        let (stop_tx, stop_rx) = channel::unbounded();
        let mut inner = SearcherImpl::new(config, database, rx, stop_rx, tx);

        let thread = thread::spawn(move || {
            let _ = inner.run();
        });

        let searcher = Self {
            thread: Some(thread),
            stop_tx,
        };

        Ok(searcher)
    }

    pub fn abort(&self) -> Result<()> {
        self.stop_tx.send(()).map_err(Into::into)
    }
}

struct SearcherImpl {
    sort_by: ColumnKind,
    sort_order: SortOrder,
    dirs_before_files: bool,
    database: Arc<Database>,
    matcher_rx: Receiver<Matcher>,
    stop_rx: Receiver<()>,
    tx: Sender<Vec<EntryId>>,
    search: Option<Search>,
}

impl SearcherImpl {
    fn new(
        config: &Config,
        database: Arc<Database>,
        matcher_rx: Receiver<Matcher>,
        stop_rx: Receiver<()>,
        tx: Sender<Vec<EntryId>>,
    ) -> Self {
        Self {
            sort_by: config.ui.sort_by,
            sort_order: config.ui.sort_order,
            dirs_before_files: config.ui.dirs_before_files,
            database,
            matcher_rx,
            stop_rx,
            tx,
            search: None,
        }
    }

    fn run(&mut self) -> Result<()> {
        loop {
            channel::select! {
                recv(self.matcher_rx) -> matcher => {
                    if let Some(search) = &self.search {
                        search.abort();
                    }

                    let matcher = matcher?;
                    if matcher.query_is_empty() {
                        let _ = self.tx.send(Vec::new());
                        continue;
                    }

                    let database = self.database.clone();
                    let tx_clone = self.tx.clone();
                    let aborted = Arc::new(AtomicBool::new(false));
                    let aborted_clone = aborted.clone();

                    let compare_func = build_compare_func(&self.sort_by, &self.sort_order, self.dirs_before_files);

                    let thread = thread::spawn(move || {
                        let hits = {
                            let result = database.abortable_search(&matcher, aborted.clone());
                            result.map(|mut hits| {
                                hits.as_parallel_slice_mut()
                                    .par_sort_unstable_by(|a, b| {
                                        compare_func(&database.entry(a), &database.entry(b))
                                    });
                                hits
                            })
                        };
                        if !aborted.load(atomic::Ordering::Relaxed) {
                            aborted.store(true, atomic::Ordering::Relaxed);
                            if let Ok(hits) = hits {
                                let _ = tx_clone.send(hits);
                            }
                        }
                    });

                    self.search.replace(Search {
                        thread: Some(thread),
                        aborted: aborted_clone,
                    });
                },
                recv(self.stop_rx) -> _ => {
                    break;
                }
            }
        }

        Ok(())
    }
}

struct Search {
    thread: Option<JoinHandle<()>>,
    aborted: Arc<AtomicBool>,
}

impl Drop for Search {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Search {
    fn abort(&self) {
        self.aborted.store(true, atomic::Ordering::Relaxed);
    }
}

fn load_database<P>(db_path: P) -> Result<Database>
where
    P: AsRef<Path>,
{
    let reader = BufReader::new(File::open(&db_path)?);
    let db: Database = bincode::deserialize_from(reader)?;
    Ok(db)
}

fn build_compare_func(
    sort_by: &ColumnKind,
    sort_order: &SortOrder,
    dirs_before_files: bool,
) -> Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> {
    let cmp_status: Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> = match sort_by {
        ColumnKind::Basename => Box::new(move |a, b| a.basename().cmp(b.basename())),
        ColumnKind::FullPath => Box::new(move |a, b| a.path().cmp(&b.path())),
        ColumnKind::Extension => Box::new(move |a, b| a.extension().cmp(&b.extension())),
        ColumnKind::Size => Box::new(move |a, b| {
            b.is_dir()
                .cmp(&a.is_dir())
                .then_with(|| a.size().cmp(&b.size()))
        }),
        ColumnKind::Mode => Box::new(move |a, b| a.mode().cmp(&b.mode())),
        ColumnKind::Created => Box::new(move |a, b| a.created().cmp(&b.created())),
        ColumnKind::Modified => Box::new(move |a, b| a.modified().cmp(&b.modified())),
        ColumnKind::Accessed => Box::new(move |a, b| a.accessed().cmp(&b.accessed())),
    };

    let cmp_file_dir: Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> =
        if dirs_before_files {
            Box::new(move |a, b| b.is_dir().cmp(&a.is_dir()))
        } else {
            Box::new(move |_, _| cmp::Ordering::Equal)
        };

    // 1. (optional) sort directories before files
    // 2. tiebreak by basename
    // 3. (optional) reverse
    match sort_order {
        SortOrder::Ascending => Box::new(move |a, b| {
            cmp_file_dir(a, b)
                .then_with(|| cmp_status(a, b).then_with(|| a.basename().cmp(b.basename())))
        }),
        SortOrder::Descending => Box::new(move |a, b| {
            cmp_file_dir(a, b)
                .then_with(|| cmp_status(b, a).then_with(|| b.basename().cmp(a.basename())))
        }),
    }
}
