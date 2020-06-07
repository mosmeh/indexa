use anyhow::Result;
use crossbeam::channel::{self, Receiver, Sender};
use indexa::{Database, Hit};
use rayon::prelude::*;
use regex::Regex;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
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
        database: Arc<Database>,
        in_path: bool,
        rx: Receiver<Regex>,
        tx: Sender<Vec<Hit>>,
    ) -> Result<Self> {
        let (stop_tx, stop_rx) = channel::unbounded();

        let mut inner = SearcherImpl {
            database,
            in_path,
            pattern_rx: rx,
            stop_rx,
            tx,
            search: None,
        };

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
    database: Arc<Database>,
    in_path: bool,
    pattern_rx: Receiver<Regex>,
    stop_rx: Receiver<()>,
    tx: Sender<Vec<Hit>>,
    search: Option<Search>,
}

impl SearcherImpl {
    fn run(&mut self) -> Result<()> {
        loop {
            channel::select! {
                recv(self.pattern_rx) -> pattern => {
                    if let Some(search) = self.search.take() {
                        search.abort();
                    }

                    let pattern = pattern?;
                    let database = self.database.clone();
                    let in_path = self.in_path;
                    let tx_clone = self.tx.clone();
                    let aborted = Arc::new(AtomicBool::new(false));
                    let aborted_clone = aborted.clone();

                    let thread = thread::spawn(move || {
                        let hits = {
                            let result = database.abortable_search(&pattern, in_path, aborted.clone());
                            result.map(|mut hits| {
                                hits.as_parallel_slice_mut()
                                    .par_sort_unstable_by_key(|hit| database.status_from_hit(hit).mtime);
                                hits
                            })
                        };
                        if !aborted.load(Ordering::Relaxed) {
                            aborted.store(true, Ordering::Relaxed);
                            if let Ok(hits) = hits {
                                tx_clone.send(hits).unwrap();
                            }
                        }
                    });

                    self.search = Some(Search {
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
        self.aborted.store(true, Ordering::Relaxed);
        let _ = self.thread.join();
    }
}
