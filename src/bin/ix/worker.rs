use indexa::database::{Database, EntryId};
use indexa::query::Query;

use anyhow::Result;
use crossbeam_channel::{self, Receiver, Sender};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::atomic::{self, AtomicBool};
use std::sync::Arc;
use std::thread;

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
                    if query.is_empty() {
                        let _ = self.tx.send(Vec::new());
                        continue;
                    }

                    let database = self.database.clone();
                    let tx_clone = self.tx.clone();
                    let aborted = Arc::new(AtomicBool::new(false));
                    let aborted_clone = aborted.clone();

                    thread::spawn(move || {
                        let hits = database.search(&query, aborted.clone());
                        if let Ok(hits) = hits {
                            if !aborted.load(atomic::Ordering::Relaxed) {
                                let _ = tx_clone.send(hits);
                            }
                        }
                    });

                    self.search.replace(Search {
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
    aborted: Arc<AtomicBool>,
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
