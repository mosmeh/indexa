mod text_box;

use crate::config::{ColumnKind, Config};
use crate::worker::{Loader, Searcher};
use anyhow::Result;
use chrono::offset::Local;
use chrono::DateTime;
use crossbeam::channel::{self, Sender};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEvent,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use indexa::mode::Mode;
use indexa::{Database, Entry, EntryId};
use regex::{Regex, RegexBuilder};
use std::io::{self, Write};
use std::sync::Arc;
use std::thread;
use text_box::{TextBox, TextBoxState};
use tui::backend::CrosstermBackend;
use tui::layout::{Constraint, Layout};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Paragraph, Row, Table, TableState, Text};
use tui::Frame;
use tui::Terminal;

#[cfg(unix)]
use crate::config::ModeFormatUnix;

#[cfg(windows)]
use crate::config::ModeFormatWindows;

pub fn run(config: &Config) -> Result<()> {
    TuiApp::new(config)?.run()
}

type Backend = CrosstermBackend<io::Stdout>;

enum State {
    Continue,
    Aborted,
    Accepted,
}

struct TuiApp<'a> {
    config: &'a Config,
    database: Option<Arc<Database>>,
    text_box_state: TextBoxState,
    hits: Vec<EntryId>,
    selected: usize,
    search_in_progress: bool,
    pattern_tx: Option<Sender<Regex>>,
}

impl<'a> TuiApp<'a> {
    fn new(config: &'a Config) -> Result<Self> {
        let app = Self {
            config,
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
        let db_path = self.config.database.location.clone();
        let loader = Loader::run(db_path, load_tx)?;

        terminal::enable_raw_mode()?;
        let mut stdout = io::stdout();
        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.hide_cursor()?;
        terminal.clear()?;

        let (input_tx, input_rx) = channel::unbounded();
        thread::spawn(move || loop {
            if let Ok(event) = event::read() {
                input_tx.send(event).unwrap();
            }
        });

        let database = loop {
            let terminal_width = terminal.size()?.width;
            terminal.draw(|mut f| self.draw(&mut f, terminal_width))?;
            channel::select! {
                recv(load_rx) -> database => break Some(database?),
                recv(input_rx) -> event => {
                    match self.handle_input(event?)? {
                        State::Aborted | State::Accepted => break None,
                        _ => (),
                    }
                }
            }
        };

        if let Some(database) = database {
            let database = Arc::new(database);
            self.database = Some(Arc::clone(&database));

            let (pattern_tx, pattern_rx) = channel::unbounded();
            let (result_tx, result_rx) = channel::unbounded();
            let searcher = Searcher::run(self.config, database, pattern_rx, result_tx)?;

            self.pattern_tx = Some(pattern_tx);
            self.on_pattern_change()?;

            loop {
                let terminal_width = terminal.size()?.width;
                terminal.draw(|mut f| self.draw(&mut f, terminal_width))?;
                channel::select! {
                    recv(result_rx) -> hits => {
                        self.handle_search_result(hits?)?;
                    }
                    recv(input_rx) -> event => {
                        match self.handle_input(event?)? {
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

    fn draw(&mut self, f: &mut Frame<Backend>, terminal_width: u16) {
        let chunks = Layout::default()
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(f.size());

        // hits table
        self.draw_table(f, chunks[0], terminal_width);

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

    fn draw_table(&self, f: &mut Frame<Backend>, area: tui::layout::Rect, terminal_width: u16) {
        let header = self
            .config
            .ui
            .columns
            .iter()
            .map(|column| format!("{}", column.kind));

        let items = self.hits.iter().map(|id| {
            let entry = self.database.as_ref().unwrap().entry(id);
            let columns = self
                .config
                .ui
                .columns
                .iter()
                .map(|column| {
                    self.display_column_content(&column.kind, &entry)
                        .unwrap_or_else(|| "".to_string())
                })
                .collect::<Vec<_>>();
            Row::Data(columns.into_iter())
        });

        let (num_fixed, sum_widths) =
            self.config
                .ui
                .columns
                .iter()
                .fold((0, 0), |(num_fixed, sum_widths), column| {
                    if let Some(width) = column.width {
                        (num_fixed + 1, sum_widths + width)
                    } else {
                        (num_fixed, sum_widths)
                    }
                });
        let remaining_width = terminal_width - sum_widths;
        let num_flexible = self.config.ui.columns.len() as u16 - num_fixed;
        let flexible_width = remaining_width / num_flexible;
        let widths = self
            .config
            .ui
            .columns
            .iter()
            .map(|column| {
                if let Some(width) = column.width {
                    Constraint::Length(width)
                } else {
                    Constraint::Min(flexible_width)
                }
            })
            .collect::<Vec<_>>();

        let table = Table::new(header, items)
            .widths(&widths)
            .highlight_style(Style::default().fg(Color::Green).modifier(Modifier::BOLD))
            .highlight_symbol("> ");

        let mut table_state = TableState::default();
        table_state.select(Some(self.selected));

        f.render_stateful_widget(table, area, &mut table_state);
    }

    fn display_column_content(&self, kind: &ColumnKind, entry: &Entry) -> Option<String> {
        match kind {
            ColumnKind::Basename => Some(entry.basename().to_string()),
            ColumnKind::FullPath => entry.path().to_str().map(|s| s.to_string()),
            ColumnKind::Extension => entry.extension().map(|s| s.to_string()),
            ColumnKind::Size => self.display_size(entry.size()),
            ColumnKind::Mode => self.display_mode(entry.mode()),
            ColumnKind::Created => self.display_datetime(entry.created()),
            ColumnKind::Modified => self.display_datetime(entry.modified()),
            ColumnKind::Accessed => self.display_datetime(entry.accessed()),
        }
    }

    fn display_size(&self, size: Option<u64>) -> Option<String> {
        size.map(|s| {
            if self.config.ui.human_readable_size {
                size::Size::Bytes(s).to_string(size::Base::Base2, size::Style::Abbreviated)
            } else {
                format!("{}", s)
            }
        })
    }

    #[cfg(unix)]
    fn display_mode(&self, mode: Option<Mode>) -> Option<String> {
        mode.map(|m| match self.config.ui.unix.mode_format {
            ModeFormatUnix::Octal => format!("{}", m.display_octal()),
            ModeFormatUnix::Symbolic => format!("{}", m.display_symbolic()),
        })
    }

    #[cfg(windows)]
    fn display_mode(&self, mode: Option<Mode>) -> Option<String> {
        mode.map(|m| match self.config.ui.windows.mode_format {
            ModeFormatWindows::Traditional => format!("{}", m.display_traditional()),
            ModeFormatWindows::PowerShell => format!("{}", m.display_powershell()),
        })
    }

    fn display_datetime(&self, time: Option<&std::time::SystemTime>) -> Option<String> {
        time.map(|t| {
            let datetime = DateTime::<Local>::from(*t);
            format!("{}", datetime.format(&self.config.ui.datetime_format))
        })
    }

    fn handle_input(&mut self, event: Event) -> Result<State> {
        match event {
            Event::Key(key) => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            Event::Resize(_, _) => Ok(State::Continue),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<State> {
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
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                self.text_box_state.clear();
                self.on_pattern_change()?;
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

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<State> {
        match mouse {
            MouseEvent::ScrollUp(_, _, _) => self.on_up()?,
            MouseEvent::ScrollDown(_, _, _) => self.on_down()?,
            _ => (),
        };
        Ok(State::Continue)
    }

    fn handle_search_result(&mut self, hits: Vec<EntryId>) -> Result<()> {
        self.hits = hits;
        self.selected = self.selected.min(self.hits.len().saturating_sub(1));
        self.search_in_progress = false;
        Ok(())
    }

    fn on_up(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.selected = self.selected.saturating_sub(1);
        }

        Ok(())
    }

    fn on_down(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.selected = (self.selected + 1).min(self.hits.len() - 1);
        }

        Ok(())
    }

    fn on_accept(&self) -> Result<()> {
        if let Some(id) = self.hits.get(self.selected) {
            println!(
                "{}",
                self.database
                    .as_ref()
                    .unwrap()
                    .entry(id)
                    .path()
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
            let regex = if self.config.flags.regex {
                RegexBuilder::new(pattern)
            } else {
                RegexBuilder::new(&regex::escape(pattern))
            }
            .case_insensitive(!self.config.flags.case_sensitive)
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
