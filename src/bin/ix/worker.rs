use indexa::{
    database::{Database, EntryId},
    query::Query,
    Error,
};

use anyhow::{Context, Result};
use bincode::Options;
use crossbeam_channel::{self, Receiver, Sender};
use std::{
    path::Path,
    sync::{
        atomic::{self, AtomicBool},
        Arc,
    },
    thread,
};

pub struct Loader;

impl Loader {
    pub fn run<P>(db_path: P, tx: Sender<Result<Database>>) -> Result<Self>
    where
        P: 'static + AsRef<Path> + Send,
    {
        thread::spawn(move || {
            let _ = tx.send(load_database(db_path));
        });

        Ok(Self)
    }
}

fn load_database<P>(db_path: P) -> Result<Database>
where
    P: AsRef<Path>,
{
    let database: Database = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .deserialize(&std::fs::read(db_path)?)
        .context("Failed to load database. Try updating the database")?;
    Ok(database)
}

pub struct Searcher {
    stop_tx: Sender<()>,
}

impl Drop for Searcher {
    fn drop(&mut self) {
        let _ = self.stop_tx.send(());
    }
}

impl Searcher {
    pub fn run(
        database: Arc<Database>,
        rx: Receiver<Query>,
        tx: Sender<Vec<EntryId>>,
    ) -> Result<Self> {
        let (stop_tx, stop_rx) = crossbeam_channel::bounded(1);
        let mut inner = SearcherImpl::new(database, rx, stop_rx, tx);
        thread::spawn(move || {
            let _ = inner.run();
        });

        Ok(Self { stop_tx })
    }
}

struct SearcherImpl {
    database: Arc<Database>,
    query_rx: Receiver<Query>,
    stop_rx: Receiver<()>,
    tx: Sender<Vec<EntryId>>,
    search: Option<Search>,
}

impl SearcherImpl {
    fn new(
        database: Arc<Database>,
        query_rx: Receiver<Query>,
        stop_rx: Receiver<()>,
        tx: Sender<Vec<EntryId>>,
    ) -> Self {
        Self {
            database,
            query_rx,
            stop_rx,
            tx,
            search: None,
        }
    }

    fn run(&mut self) -> Result<()> {
        loop {
            crossbeam_channel::select! {
                recv(self.query_rx) -> query => {
                    if let Some(search) = &self.search {
                        search.abort();
                    }

                    let query = query?;
                    let database = self.database.clone();
                    let tx_clone = self.tx.clone();
                    let abort_signal = Arc::new(AtomicBool::new(false));
                    let abort_signal_clone = abort_signal.clone();

                    thread::spawn(move || {
                        let hits = database.abortable_search(&query, &abort_signal);
                        match hits {
                            Ok(hits) => {
                                if !abort_signal.load(atomic::Ordering::Relaxed) {
                                    let _ = tx_clone.send(hits);
                                }
                            }
                            Err(Error::SearchAbort) => (),
                            Err(e) => panic!("{}", e),
                        }
                    });

                    self.search.replace(Search {
                        abort_signal: abort_signal_clone,
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
    abort_signal: Arc<AtomicBool>,
}

impl Search {
    fn abort(&self) {
        self.abort_signal.store(true, atomic::Ordering::Relaxed);
    }
}
