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
    thread: JoinHandle<()>,
}

impl Loader {
    pub fn run<P>(db_path: P, tx: Sender<Result<Database>>) -> Result<Self>
    where
        P: 'static + AsRef<Path> + Send,
    {
        let thread = thread::spawn(move || {
            let _ = tx.send(load_database(db_path));
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
        rx: Receiver<Matcher>,
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
    sort_by: ColumnKind,
    sort_order: SortOrder,
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
                    if let Some(search) = self.search.take() {
                        search.abort();
                    }

                    let matcher = matcher?;
                    let database = self.database.clone();
                    let tx_clone = self.tx.clone();
                    let aborted = Arc::new(AtomicBool::new(false));
                    let aborted_clone = aborted.clone();

                    let compare_func = build_compare_func(&self.sort_by, &self.sort_order);

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
) -> Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> {
    let compare: Box<dyn Fn(&Entry, &Entry) -> cmp::Ordering + Send + Sync> = match sort_by {
        ColumnKind::Basename => Box::new(move |_, _| cmp::Ordering::Equal),
        ColumnKind::FullPath => Box::new(move |a, b| a.path().cmp(&b.path())),
        ColumnKind::Extension => Box::new(move |a, b| a.extension().cmp(&b.extension())),
        ColumnKind::Size => Box::new(move |a, b| a.size().cmp(&b.size())),
        ColumnKind::Mode => Box::new(move |a, b| a.mode().cmp(&b.mode())),
        ColumnKind::Created => Box::new(move |a, b| a.created().cmp(&b.created())),
        ColumnKind::Modified => Box::new(move |a, b| a.modified().cmp(&b.modified())),
        ColumnKind::Accessed => Box::new(move |a, b| a.accessed().cmp(&b.accessed())),
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
