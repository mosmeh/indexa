use crate::config::{ColumnType, Config, SortOrder};
use anyhow::Result;
use crossbeam::channel::{self, Receiver, Sender};
use indexa::{Database, Entry, EntryId};
use rayon::prelude::*;
use regex::Regex;
use std::cmp;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{self, AtomicBool};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

pub struct Loader {
    thread: JoinHandle<()>,
}

impl Loader {
    pub fn run<P>(db_path: P, tx: Sender<Database>) -> Result<Self>
    where
        P: 'static + AsRef<Path> + Send,
    {
        let thread = thread::spawn(move || {
            let reader = BufReader::new(File::open(&db_path).unwrap());
            let database = bincode::deserialize_from(reader).unwrap();
            tx.send(database).unwrap();
        });

        let loader = Self { thread };

        Ok(loader)
    }

    pub fn finish(self) -> Result<()> {
        let _ = self.thread.join();
        Ok(())
    }
}

pub struct Searcher {
    thread: JoinHandle<()>,
    stop_tx: Sender<()>,
}

impl Searcher {
    pub fn run(
        config: &Config,
        database: Arc<Database>,
        rx: Receiver<Regex>,
        tx: Sender<Vec<EntryId>>,
    ) -> Result<Self> {
        let (stop_tx, stop_rx) = channel::unbounded();
        let mut inner = SearcherImpl::new(config, database, rx, stop_rx, tx);

        let thread = thread::spawn(move || {
            let _ = inner.run();
        });

        let searcher = Self { thread, stop_tx };

        Ok(searcher)
    }

    pub fn finish(self) -> Result<()> {
        self.stop_tx.send(())?;
        let _ = self.thread.join();
        Ok(())
    }
}

struct SearcherImpl {
    in_path: bool,
    sort_by: ColumnType,
    sort_order: SortOrder,
    database: Arc<Database>,
    pattern_rx: Receiver<Regex>,
    stop_rx: Receiver<()>,
    tx: Sender<Vec<EntryId>>,
    search: Option<Search>,
}

impl SearcherImpl {
    fn new(
        config: &Config,
        database: Arc<Database>,
        pattern_rx: Receiver<Regex>,
        stop_rx: Receiver<()>,
        tx: Sender<Vec<EntryId>>,
    ) -> Self {
        Self {
            in_path: config.flags.in_path,
            sort_by: config.ui.sort_by,
            sort_order: config.ui.sort_order,
            database,
            pattern_rx,
            stop_rx,
            tx,
            search: None,
        }
    }

    fn run(&mut self) -> Result<()> {
        loop {
            channel::select! {
                recv(self.pattern_rx) -> pattern => {
                    if let Some(search) = self.search.take() {
                        search.abort();
                    }

                    let pattern = pattern?;
                    let database = self.database.clone();
                    let tx_clone = self.tx.clone();
                    let aborted = Arc::new(AtomicBool::new(false));
                    let aborted_clone = aborted.clone();

                    let in_path = self.in_path;
                    let compare_func = build_compare_func(&self.sort_by, &self.sort_order);

                    let thread = thread::spawn(move || {
                        let hits = {
                            let result = database.abortable_search(&pattern, in_path, aborted.clone());
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
                                tx_clone.send(hits).unwrap();
                            }
                        }
                    });

                    self.search.replace(Search {
                        thread,
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
    thread: JoinHandle<()>,
    aborted: Arc<AtomicBool>,
}

impl Search {
    fn abort(self) {
        self.aborted.store(true, atomic::Ordering::Relaxed);
        let _ = self.thread.join();
    }
}

fn build_compare_func(
    sort_by: &ColumnType,
    sort_order: &SortOrder,
) -> Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> {
    let compare: Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> = match sort_by {
        ColumnType::Basename => Box::new(move |_, _| cmp::Ordering::Equal),
        ColumnType::FullPath => Box::new(move |a, b| a.path().cmp(&b.path())),
        ColumnType::Extension => Box::new(move |a, b| a.extension().cmp(&b.extension())),
        ColumnType::Size => Box::new(move |a, b| a.size().cmp(&b.size())),
        ColumnType::Mode => Box::new(move |a, b| a.mode().cmp(&b.mode())),
        ColumnType::Created => Box::new(move |a, b| a.created().cmp(&b.created())),
        ColumnType::Modified => Box::new(move |a, b| a.modified().cmp(&b.modified())),
        ColumnType::Accessed => Box::new(move |a, b| a.accessed().cmp(&b.accessed())),
    };

    // tiebreak by basename and (optionally) reverse
    match sort_order {
        SortOrder::Ascending => {
            Box::new(move |a, b| compare(a, b).then_with(|| a.basename().cmp(b.basename())))
        }
        SortOrder::Descending => {
            Box::new(move |a, b| compare(b, a).then_with(|| b.basename().cmp(a.basename())))
        }
    }
}
