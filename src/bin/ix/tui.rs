mod table;
mod text_box;

use crate::config::{Config, SortOrder};
use crate::worker::{Loader, Searcher};

use table::{HighlightableText, Row, Table, TableState};
use text_box::{TextBox, TextBoxState};

use indexa::database::{Database, Entry, EntryId, StatusKind};
use indexa::matcher::{MatchDetail, Matcher, MatcherBuilder};
use indexa::mode::Mode;

use anyhow::{anyhow, Result};
use chrono::offset::Local;
use chrono::DateTime;
use crossbeam::channel::{self, Sender};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEvent,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use std::borrow::Cow;
use std::io::{self, Write};
use std::ops::Range;
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;
use tui::backend::CrosstermBackend;
use tui::layout::{Alignment, Constraint, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Paragraph, Text};
use tui::Frame;
use tui::Terminal;

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
    matcher: Option<Matcher>,
    hits: Vec<EntryId>,
    search_in_progress: bool,
    matcher_tx: Option<Sender<Matcher>>,
    text_box_state: TextBoxState,
    table_state: TableState,
    page_shift_amount: u16,
}

impl<'a> TuiApp<'a> {
    fn new(config: &'a Config) -> Result<Self> {
        let app = Self {
            config,
            database: None,
            matcher: None,
            hits: Vec::new(),
            search_in_progress: false,
            matcher_tx: None,
            text_box_state: Default::default(),
            table_state: Default::default(),
            page_shift_amount: 0,
        };

        Ok(app)
    }

    fn run(&mut self) -> Result<()> {
        let (load_tx, load_rx) = channel::unbounded();
        let db_path = self.config.database.location.as_ref().unwrap().clone();
        let _loader = Loader::run(db_path, load_tx)?;

        let mut terminal = setup_terminal()?;

        let (input_tx, input_rx) = channel::unbounded();
        thread::spawn(move || loop {
            if let Ok(event) = event::read() {
                let _ = input_tx.send(event);
            }
        });

        let database = loop {
            let terminal_width = terminal.size()?.width;
            terminal.draw(|mut f| self.draw(&mut f, terminal_width))?;
            channel::select! {
                recv(load_rx) -> database => break Some(database??),
                recv(input_rx) -> event => {
                    match self.handle_input(event?)? {
                        State::Aborted | State::Accepted => break None,
                        _ => (),
                    }
                }
            }
        };

        if let Some(database) = database {
            if !database.is_indexed(self.config.ui.sort_by) {
                cleanup_terminal(&mut terminal)?;
                return Err(anyhow!(
                    "You cannot sort by a non-indexed status. \
                Please edit the config file and/or update the database."
                ));
            }

            let database = Arc::new(database);
            self.database = Some(Arc::clone(&database));

            let (matcher_tx, matcher_rx) = channel::unbounded();
            let (result_tx, result_rx) = channel::unbounded();
            let searcher = Searcher::run(self.config, database, matcher_rx, result_tx)?;

            self.matcher_tx = Some(matcher_tx);
            self.on_query_change()?;

            loop {
                let terminal_width = terminal.size()?.width;
                terminal.draw(|mut f| self.draw(&mut f, terminal_width))?;
                channel::select! {
                    recv(result_rx) -> hits => {
                        self.handle_search_result(hits?)?;
                    }
                    recv(input_rx) -> event => {
                        match self.handle_input(event?)? {
                            State::Aborted => break,
                            State::Accepted => {
                                cleanup_terminal(&mut terminal)?;
                                self.on_accept()?;
                                break;
                            }
                            _ => (),
                        }
                    }
                }
            }

            searcher.abort()?;
        }

        cleanup_terminal(&mut terminal)?;

        Ok(())
    }

    fn draw(&mut self, f: &mut Frame<Backend>, terminal_width: u16) {
        let chunks = Layout::default()
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
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

        // path of selected row
        let text = vec![Text::raw(
            self.hits
                .get(self.table_state.selected())
                .and_then(|id| {
                    self.database
                        .as_ref()
                        .unwrap()
                        .entry(id)
                        .path()
                        .to_str()
                        .map(|s| s.to_string())
                })
                .unwrap_or_else(|| "".to_string()),
        )];
        let paragraph = Paragraph::new(text.iter());
        f.render_widget(paragraph, chunks[2]);

        // input box
        let text_box = TextBox::new()
            .highlight_style(Style::default().fg(Color::Black).bg(Color::White))
            .prompt(Text::styled(
                "> ",
                Style::default().fg(Color::Blue).modifier(Modifier::BOLD),
            ));
        f.render_stateful_widget(text_box, chunks[3], &mut self.text_box_state);
    }

    fn draw_table(&mut self, f: &mut Frame<Backend>, area: Rect, terminal_width: u16) {
        let columns = &self.config.ui.columns;

        let header = columns.iter().map(|column| {
            if column.status == self.config.ui.sort_by {
                match self.config.ui.sort_order {
                    SortOrder::Ascending => format!("{}▲", column.status),
                    SortOrder::Descending => format!("{}▼", column.status),
                }
            } else {
                format!("{}", column.status)
            }
        });

        let items = self.hits.iter().map(|id| {
            let entry = self.database.as_ref().unwrap().entry(id);
            let match_detail = self.matcher.as_ref().unwrap().match_detail(&entry).unwrap();
            let contents = columns
                .iter()
                .map(|column| {
                    self.display_column_content(&column.status, &entry, &match_detail)
                        .unwrap_or_else(|| HighlightableText::Raw("".to_string()))
                })
                .collect::<Vec<_>>();
            Row::new(contents.into_iter())
        });

        let (num_fixed, sum_widths) =
            columns
                .iter()
                .fold((0, 0), |(num_fixed, sum_widths), column| {
                    if let Some(width) = column.width {
                        (num_fixed + 1, sum_widths + width)
                    } else {
                        (num_fixed, sum_widths)
                    }
                });
        let remaining_width = terminal_width - sum_widths;
        let num_flexible = columns.len() as u16 - num_fixed;
        let flexible_width = remaining_width.checked_div(num_flexible);
        let widths = columns
            .iter()
            .map(|column| {
                if let Some(width) = column.width {
                    Constraint::Length(width)
                } else {
                    Constraint::Min(flexible_width.unwrap())
                }
            })
            .collect::<Vec<_>>();

        let alignments = columns
            .iter()
            .map(|column| match column.status {
                StatusKind::Size => Alignment::Right,
                _ => Alignment::Left,
            })
            .collect::<Vec<_>>();

        let table = Table::new(header, items)
            .num_rows(self.hits.len())
            .widths(&widths)
            .alignments(&alignments)
            .selected_style(Style::default().fg(Color::Blue))
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Blue))
            .selected_highlight_style(Style::default().fg(Color::Black).bg(Color::Blue))
            .selected_symbol("> ")
            .header_gap(1);

        let mut table_state = self.table_state.clone();
        f.render_stateful_widget(table, area, &mut table_state);
        self.table_state = table_state;

        self.page_shift_amount = area.height.saturating_sub(
            // header
            1 +
            // header_gap
            1 +
            // one less than page height
            1,
        );
    }

    fn display_column_content(
        &self,
        kind: &StatusKind,
        entry: &Entry,
        match_detail: &MatchDetail,
    ) -> Option<HighlightableText<impl Iterator<Item = Range<usize>>>> {
        match kind {
            StatusKind::Basename => Some(HighlightableText::Highlighted(
                entry.basename().to_string(),
                match_detail.basename_matches().into_iter(),
            )),

            StatusKind::FullPath => Some(HighlightableText::Highlighted(
                match_detail.path_str().to_string(),
                match_detail.path_matches().into_iter(),
            )),
            StatusKind::Extension => entry
                .extension()
                .map(|s| HighlightableText::Raw(s.to_string())),
            StatusKind::Size => self
                .display_size(entry.size(), entry.is_dir())
                .map(HighlightableText::Raw),
            StatusKind::Mode => self.display_mode(entry.mode()).map(HighlightableText::Raw),
            StatusKind::Created => self
                .display_datetime(entry.created())
                .map(HighlightableText::Raw),
            StatusKind::Modified => self
                .display_datetime(entry.modified())
                .map(HighlightableText::Raw),
            StatusKind::Accessed => self
                .display_datetime(entry.accessed())
                .map(HighlightableText::Raw),
        }
    }

    fn display_size(&self, size: Option<u64>, is_dir: bool) -> Option<String> {
        size.map(|s| {
            if is_dir {
                if s == 1 {
                    format!("{} item", s)
                } else {
                    format!("{} items", s)
                }
            } else if self.config.ui.human_readable_size {
                size::Size::Bytes(s).to_string(size::Base::Base2, size::Style::Abbreviated)
            } else {
                format!("{}", s)
            }
        })
    }

    #[cfg(unix)]
    fn display_mode(&self, mode: Option<Mode>) -> Option<String> {
        use crate::config::ModeFormatUnix;

        mode.map(|m| match self.config.ui.unix.mode_format {
            ModeFormatUnix::Octal => format!("{}", m.display_octal()),
            ModeFormatUnix::Symbolic => format!("{}", m.display_symbolic()),
        })
    }

    #[cfg(windows)]
    fn display_mode(&self, mode: Option<Mode>) -> Option<String> {
        use crate::config::ModeFormatWindows;

        mode.map(|m| match self.config.ui.windows.mode_format {
            ModeFormatWindows::Traditional => format!("{}", m.display_traditional()),
            ModeFormatWindows::PowerShell => format!("{}", m.display_powershell()),
        })
    }

    fn display_datetime(&self, time: Option<Cow<'_, SystemTime>>) -> Option<String> {
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
                    self.on_query_change()?;
                }
            }
            (_, KeyCode::Delete) | (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                if self.text_box_state.on_delete() {
                    self.on_query_change()?;
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
                self.on_query_change()?;
            }
            (_, KeyCode::Up)
            | (KeyModifiers::CONTROL, KeyCode::Char('p'))
            | (KeyModifiers::CONTROL, KeyCode::Char('k')) => self.on_up()?,
            (_, KeyCode::Down)
            | (KeyModifiers::CONTROL, KeyCode::Char('n'))
            | (KeyModifiers::CONTROL, KeyCode::Char('j')) => self.on_down()?,
            (_, KeyCode::PageUp) => self.on_pageup()?,
            (_, KeyCode::PageDown) => self.on_pagedown()?,
            (_, KeyCode::Char(c)) => {
                self.text_box_state.on_char(c);
                self.on_query_change()?;
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
        self.search_in_progress = false;

        if !self.hits.is_empty() {
            self.table_state
                .select(self.table_state.selected().min(self.hits.len() - 1));
        }

        Ok(())
    }

    fn on_up(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.table_state
                .select(self.table_state.selected().saturating_sub(1));
        }

        Ok(())
    }

    fn on_down(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.table_state
                .select((self.table_state.selected() + 1).min(self.hits.len() - 1));
        }

        Ok(())
    }

    fn on_pageup(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.table_state.select(
                self.table_state
                    .selected()
                    .saturating_sub(self.page_shift_amount as usize),
            );
        }

        Ok(())
    }

    fn on_pagedown(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.table_state.select(
                (self.table_state.selected() + self.page_shift_amount as usize)
                    .min(self.hits.len() - 1),
            );
        }

        Ok(())
    }

    fn on_accept(&self) -> Result<()> {
        if let Some(id) = self.hits.get(self.table_state.selected()) {
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

    fn on_query_change(&mut self) -> Result<()> {
        if self.database.is_none() {
            return Ok(());
        }

        let query = self.text_box_state.text();
        let matcher = MatcherBuilder::new(query)
            .match_path(self.config.flags.match_path)
            .auto_match_path(self.config.flags.auto_match_path)
            .case_insensitive(!self.config.flags.case_sensitive)
            .regex(self.config.flags.regex)
            .build();

        if let Ok(matcher) = matcher {
            self.matcher = Some(matcher.clone());
            self.search_in_progress = true;
            self.matcher_tx.as_ref().unwrap().send(matcher)?;
        }

        Ok(())
    }
}

fn setup_terminal() -> Result<Terminal<Backend>> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
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
