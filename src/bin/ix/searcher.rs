use indexa::{
    database::{Database, EntryId},
    query::Query,
    Error,
};

use crossbeam_channel::Sender;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

pub struct Searcher {
    database: Arc<Database>,
    tx: Sender<Vec<EntryId>>,
    search: Option<Search>,
}

impl Searcher {
    pub fn new(database: Arc<Database>, tx: Sender<Vec<EntryId>>) -> Self {
        Self {
            database,
            tx,
            search: None,
        }
    }

    pub fn search(&mut self, query: Query) {
        if let Some(search) = &self.search {
            search.abort();
        }

        let abort_signal = Arc::new(AtomicBool::new(false));

        {
            let database = self.database.clone();
            let tx = self.tx.clone();
            let abort_signal = abort_signal.clone();

            thread::spawn(move || {
                let hits = database.abortable_search(&query, &abort_signal);
                match hits {
                    Ok(hits) => {
                        if !abort_signal.load(Ordering::Relaxed) {
                            let _ = tx.send(hits);
                        }
                    }
                    Err(Error::SearchAbort) => (),
                    Err(e) => panic!("{}", e),
                }
            });
        }

        self.search = Some(Search { abort_signal });
    }
}

struct Search {
    abort_signal: Arc<AtomicBool>,
}

impl Drop for Search {
    fn drop(&mut self) {
        self.abort();
    }
}

impl Search {
    fn abort(&self) {
        self.abort_signal.store(true, Ordering::Relaxed);
    }
}
