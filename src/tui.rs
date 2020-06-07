mod text_box;

use crate::worker::{Loader, Searcher};
use crate::Opt;
use anyhow::Result;
use crossbeam::channel::{self, Sender};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use indexa::{Database, Hit};
use regex::{Regex, RegexBuilder};
use std::io::{self, Write};
use std::sync::Arc;
use std::thread;
use text_box::{TextBox, TextBoxState};
use tui::backend::CrosstermBackend;
use tui::layout::{Constraint, Layout};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{List, ListState, Paragraph, Text};
use tui::Frame;
use tui::Terminal;

pub fn run(opt: &Opt) -> Result<()> {
    TuiApp::new(opt)?.run()
}

type Backend = CrosstermBackend<io::Stdout>;

enum State {
    Continue,
    Aborted,
    Accepted,
}

struct TuiApp<'a> {
    opt: &'a Opt,
    database: Option<Arc<Database>>,
    text_box_state: TextBoxState,
    hits: Vec<Hit>,
    selected: usize,
    search_in_progress: bool,
    pattern_tx: Option<Sender<Regex>>,
}

impl<'a> TuiApp<'a> {
    fn new(opt: &'a Opt) -> Result<Self> {
        let app = Self {
            opt,
            database: None,
            text_box_state: Default::default(),
            hits: Vec::new(),
            selected: 0,
            search_in_progress: false,
            pattern_tx: None,
        };
        Ok(app)
    }

    fn run(&mut self) -> Result<()> {
        let (load_tx, load_rx) = channel::unbounded();
        let db_path = self.opt.database.clone();
        let loader = Loader::run(db_path, load_tx)?;

        let (input_tx, input_rx) = channel::unbounded();
        thread::spawn(move || loop {
            if let Ok(Event::Key(key)) = event::read() {
                input_tx.send(key).unwrap();
            }
        });

        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        terminal.clear()?;

        let database = loop {
            terminal.draw(|mut f| self.draw(&mut f))?;
            channel::select! {
                recv(load_rx) -> database => break Some(database?),
                recv(input_rx) -> key => {
                    match self.handle_input(key?)? {
                        State::Aborted | State::Accepted => break None,
                        _ => (),
                    }
                }
            }
        };

        if let Some(database) = database {
            let database = Arc::new(database);
            self.database = Some(Arc::clone(&database));

            let in_path = self.opt.in_path;
            let (pattern_tx, pattern_rx) = channel::unbounded();
            let (result_tx, result_rx) = channel::unbounded();
            let searcher = Searcher::run(database, in_path, pattern_rx, result_tx)?;

            self.pattern_tx = Some(pattern_tx);
            self.on_pattern_change()?;

            loop {
                terminal.draw(|mut f| self.draw(&mut f))?;
                channel::select! {
                    recv(result_rx) -> hits => {
                        self.handle_search_result(hits?)?;
                    }
                    recv(input_rx) -> key => {
                        match self.handle_input(key?)? {
                            State::Aborted => {
                                cleanup_terminal(terminal)?;
                                break;
                            }
                             State::Accepted => {
                                cleanup_terminal(terminal)?;
                                self.on_accept()?;
                                break;
                             }
                            _ => (),
                        }
                    }
                }
            }

            searcher.finish()?;
        }

        loader.finish()?;

        Ok(())
    }

    fn draw(&mut self, f: &mut Frame<Backend>) {
        let chunks = Layout::default()
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(f.size());

        // hits list
        let items = self.hits.iter().filter_map(|hit| {
            self.database
                .as_ref()
                .unwrap()
                .path_from_hit(hit)
                .to_str()
                .map(|path| Text::raw(path.to_string()))
        });
        let list = List::new(items)
            .highlight_style(Style::default().fg(Color::Green).modifier(Modifier::BOLD))
            .highlight_symbol("> ");
        let mut list_state = ListState::default();
        list_state.select(Some(self.selected));
        f.render_stateful_widget(list, chunks[0], &mut list_state);

        // status bar
        let text = [if self.database.is_none() {
            Text::raw("Loading database")
        } else if self.search_in_progress {
            Text::raw("Searching")
        } else {
            Text::raw(format!(
                "{} / {}",
                self.hits.len(),
                self.database.as_ref().unwrap().num_entries()
            ))
        }];
        let paragraph = Paragraph::new(text.iter());
        f.render_widget(paragraph, chunks[1]);

        // input box
        let text_box = TextBox::new()
            .highlight_style(Style::default().fg(Color::Black).bg(Color::White))
            .prompt(Text::styled(
                "> ",
                Style::default().fg(Color::Green).modifier(Modifier::BOLD),
            ));
        f.render_stateful_widget(text_box, chunks[2], &mut self.text_box_state);
    }

    fn handle_input(&mut self, key: KeyEvent) -> Result<State> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc)
            | (KeyModifiers::CONTROL, KeyCode::Char('c'))
            | (KeyModifiers::CONTROL, KeyCode::Char('g')) => return Ok(State::Aborted),
            (_, KeyCode::Enter) => return Ok(State::Accepted),
            (_, KeyCode::Backspace) | (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                if self.text_box_state.on_backspace() {
                    self.on_pattern_change()?;
                }
            }
            (_, KeyCode::Delete) | (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                if self.text_box_state.on_delete() {
                    self.on_pattern_change()?;
                }
            }
            (_, KeyCode::Left) | (KeyModifiers::CONTROL, KeyCode::Char('b')) => {
                self.text_box_state.on_left();
            }
            (_, KeyCode::Right) | (KeyModifiers::CONTROL, KeyCode::Char('f')) => {
                self.text_box_state.on_right();
            }
            (_, KeyCode::Home) | (KeyModifiers::CONTROL, KeyCode::Char('a')) => {
                self.text_box_state.on_home();
            }
            (_, KeyCode::End) | (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                self.text_box_state.on_end();
            }
            (_, KeyCode::Up)
            | (KeyModifiers::CONTROL, KeyCode::Char('p'))
            | (KeyModifiers::CONTROL, KeyCode::Char('k')) => self.on_up()?,
            (_, KeyCode::Down)
            | (KeyModifiers::CONTROL, KeyCode::Char('n'))
            | (KeyModifiers::CONTROL, KeyCode::Char('j')) => self.on_down()?,
            (_, KeyCode::Char(c)) => {
                self.text_box_state.on_char(c);
                self.on_pattern_change()?;
            }
            _ => (),
        }
        Ok(State::Continue)
    }

    fn handle_search_result(&mut self, hits: Vec<Hit>) -> Result<()> {
        self.hits = hits;
        self.selected = self.selected.min(self.hits.len().saturating_sub(1));
        self.search_in_progress = false;
        Ok(())
    }

    fn on_up(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.selected = if self.selected == 0 {
                self.hits.len() - 1
            } else {
                self.selected - 1
            };
        }

        Ok(())
    }

    fn on_down(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.selected = if self.selected >= self.hits.len() - 1 {
                0
            } else {
                self.selected + 1
            };
        }

        Ok(())
    }

    fn on_accept(&self) -> Result<()> {
        if let Some(hit) = self.hits.get(self.selected) {
            println!(
                "{}",
                self.database
                    .as_ref()
                    .unwrap()
                    .path_from_hit(hit)
                    .to_str()
                    .unwrap()
            );
        }
        Ok(())
    }

    fn on_pattern_change(&mut self) -> Result<()> {
        if self.database.is_none() {
            return Ok(());
        }

        let pattern = self.text_box_state.text();
        if pattern.is_empty() {
            self.hits.clear();
        } else {
            self.search_in_progress = true;

            let regex = if self.opt.regex {
                RegexBuilder::new(pattern)
            } else {
                RegexBuilder::new(&regex::escape(pattern))
            }
            .case_insensitive(!self.opt.case_sensitive)
            .build();

            if let Ok(pattern) = regex {
                self.search_in_progress = true;
                self.pattern_tx.as_ref().unwrap().send(pattern)?;
            }
        }

        Ok(())
    }
}

fn cleanup_terminal(mut terminal: Terminal<Backend>) -> Result<()> {
    terminal.show_cursor()?;
    terminal::disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    Ok(())
}
