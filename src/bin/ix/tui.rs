mod backend;
mod draw;
mod handlers;
mod table;
mod text_box;

use backend::CustomBackend;
use table::TableState;
use text_box::TextBoxState;

use crate::{config::Config, searcher::Searcher};

use indexa::{
    database::{Database, EntryId},
    query::Query,
};

use anyhow::{Context, Result};
use bincode::Options;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{io, path::Path, sync::Arc, thread};
use tui::Terminal;

pub fn run(config: &Config) -> Result<()> {
    TuiApp::new(config)?.run()
}

type Backend = CustomBackend<io::Stderr>;

enum State {
    Loading,
    Ready,
    Searching,
    InvalidQuery(String),
    Aborted,
    Accepted,
}

struct TuiApp<'a> {
    config: &'a Config,
    status: State,
    database: Option<Arc<Database>>,
    searcher: Option<Searcher>,
    query: Option<Query>,
    hits: Vec<EntryId>,
    text_box_state: TextBoxState,
    table_state: TableState,
    page_scroll_amount: u16,
}

impl<'a> TuiApp<'a> {
    fn new(config: &'a Config) -> Result<Self> {
        let app = Self {
            config,
            status: State::Loading,
            database: None,
            searcher: None,
            query: None,
            hits: Vec::new(),
            text_box_state: TextBoxState::with_text(
                config.flags.query.clone().unwrap_or_else(|| "".to_string()),
            ),
            table_state: Default::default(),
            page_scroll_amount: 0,
        };

        Ok(app)
    }

    fn run(&mut self) -> Result<()> {
        let (load_tx, load_rx) = crossbeam_channel::bounded(1);
        let db_path = self.config.database.location.as_ref().unwrap().clone();

        thread::spawn(move || {
            load_tx.send(load_database(db_path)).unwrap();
        });

        let mut terminal = setup_terminal()?;

        let (input_tx, input_rx) = crossbeam_channel::unbounded();
        thread::spawn(move || loop {
            if let Ok(event) = event::read() {
                let _ = input_tx.send(event);
            }
        });

        let database = loop {
            let terminal_width = terminal.size()?.width;
            terminal.draw(|f| self.draw(f, terminal_width))?;

            crossbeam_channel::select! {
                recv(load_rx) -> database => {
                    self.status = State::Ready;
                    break Some(database??);
                },
                recv(input_rx) -> event => self.handle_input(event?)?,
            }

            match self.status {
                State::Aborted | State::Accepted => {
                    cleanup_terminal(&mut terminal)?;
                    break None;
                }
                _ => (),
            }
        };

        if let Some(database) = database {
            let database = Arc::new(database);
            self.database = Some(Arc::clone(&database));

            let (result_tx, result_rx) = crossbeam_channel::bounded(1);
            self.searcher = Some(Searcher::new(database, result_tx));

            self.handle_query_change()?;

            loop {
                let terminal_width = terminal.size()?.width;
                terminal.draw(|f| self.draw(f, terminal_width))?;

                crossbeam_channel::select! {
                    recv(result_rx) -> hits => self.handle_search_result(hits?)?,
                    recv(input_rx) -> event => self.handle_input(event?)?,
                }

                match self.status {
                    State::Aborted => {
                        cleanup_terminal(&mut terminal)?;
                        break;
                    }
                    State::Accepted => {
                        cleanup_terminal(&mut terminal)?;
                        self.handle_accept()?;
                        break;
                    }
                    _ => (),
                }
            }
        }

        Ok(())
    }
}

fn setup_terminal() -> Result<Terminal<Backend>> {
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    crossterm::execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CustomBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;
    terminal.clear()?;

    Ok(terminal)
}

fn cleanup_terminal(terminal: &mut Terminal<Backend>) -> Result<()> {
    terminal.show_cursor()?;
    terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
}

fn load_database<P>(path: P) -> Result<Database>
where
    P: AsRef<Path>,
{
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
        .deserialize(&std::fs::read(path)?)
        .context("Failed to load database. Try updating the database")
}
