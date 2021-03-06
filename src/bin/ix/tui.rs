mod table;
mod text_box;

use crate::config::Config;
use crate::worker::{Loader, Searcher};

use table::{HighlightableText, Row, Table, TableState};
use text_box::{TextBox, TextBoxState};

use indexa::database::{Database, Entry, EntryId, StatusKind};
use indexa::mode::Mode;
use indexa::query::{MatchDetail, Query, QueryBuilder, SortOrder};

use anyhow::Result;
use chrono::offset::Local;
use chrono::DateTime;
use crossbeam_channel::{self, Sender};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEvent,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use std::io::{self, Write};
use std::ops::Range;
use std::sync::Arc;
use std::thread;
use std::time::SystemTime;
use tui::backend::CrosstermBackend;
use tui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{Paragraph, Text};
use tui::Frame;
use tui::Terminal;

pub fn run(config: &Config) -> Result<()> {
    TuiApp::new(config)?.run()
}

type Backend = CrosstermBackend<io::Stderr>;

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
    query: Option<Query>,
    hits: Vec<EntryId>,
    query_tx: Option<Sender<Query>>,
    text_box_state: TextBoxState,
    table_state: TableState,
    page_shift_amount: u16,
}

impl<'a> TuiApp<'a> {
    fn new(config: &'a Config) -> Result<Self> {
        let app = Self {
            config,
            status: State::Loading,
            database: None,
            query: None,
            hits: Vec::new(),
            query_tx: None,
            text_box_state: TextBoxState::with_text(
                config.flags.query.clone().unwrap_or_else(|| "".to_string()),
            ),
            table_state: Default::default(),
            page_shift_amount: 0,
        };

        Ok(app)
    }

    fn run(&mut self) -> Result<()> {
        let (load_tx, load_rx) = crossbeam_channel::bounded(1);
        let db_path = self.config.database.location.as_ref().unwrap().clone();
        let _loader = Loader::run(db_path, load_tx)?;

        let mut terminal = setup_terminal()?;

        let (input_tx, input_rx) = crossbeam_channel::unbounded();
        thread::spawn(move || loop {
            if let Ok(event) = event::read() {
                let _ = input_tx.send(event);
            }
        });

        let database = loop {
            let terminal_width = terminal.size()?.width;
            terminal.draw(|mut f| self.draw(&mut f, terminal_width))?;

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

            let (query_tx, query_rx) = crossbeam_channel::unbounded();
            let (result_tx, result_rx) = crossbeam_channel::bounded(1);
            let _searcher = Searcher::run(database, query_rx, result_tx)?;

            self.query_tx = Some(query_tx);
            self.on_query_change()?;

            loop {
                let terminal_width = terminal.size()?.width;
                terminal.draw(|mut f| self.draw(&mut f, terminal_width))?;

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
                        self.on_accept()?;
                        break;
                    }
                    _ => (),
                }
            }
        }

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
        self.draw_status_bar(f, chunks[1]);

        // path of selected row
        let text = vec![Text::raw(
            self.hits
                .get(self.table_state.selected())
                .and_then(|id| {
                    self.database
                        .as_ref()
                        .unwrap()
                        .entry(*id)
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
                Style::default()
                    .fg(self.config.ui.colors.prompt)
                    .modifier(Modifier::BOLD),
            ));
        f.render_stateful_widget(text_box, chunks[3], &mut self.text_box_state);
    }

    #[allow(clippy::needless_collect)] // false positive
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

        let display_func = |id: &EntryId| {
            let entry = self.database.as_ref().unwrap().entry(*id);
            let match_detail = self.query.as_ref().unwrap().match_detail(&entry).unwrap();
            let contents = columns
                .iter()
                .map(|column| {
                    self.display_column_content(&column.status, &entry, &match_detail)
                        .unwrap_or_else(|| HighlightableText::Raw("".to_string()))
                })
                .collect::<Vec<_>>();
            Row::new(contents.into_iter())
        };

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

        let table = Table::new(header, self.hits.iter(), display_func)
            .widths(&widths)
            .alignments(&alignments)
            .selected_style(
                Style::default()
                    .fg(self.config.ui.colors.selected_fg)
                    .bg(self.config.ui.colors.selected_bg),
            )
            .highlight_style(
                Style::default()
                    .fg(self.config.ui.colors.matched_fg)
                    .bg(self.config.ui.colors.matched_bg),
            )
            .selected_highlight_style(
                Style::default()
                    .fg(self.config.ui.colors.matched_fg)
                    .bg(self.config.ui.colors.matched_bg),
            )
            .selected_symbol("> ")
            .header_gap(1)
            .column_spacing(self.config.ui.column_spacing);

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

    fn draw_status_bar(&self, f: &mut Frame<Backend>, area: Rect) {
        let message = match &self.status {
            State::Loading => Text::raw("Loading database"),
            State::Searching => Text::raw("Searching"),
            State::Ready | State::Aborted | State::Accepted => Text::raw("Ready"),
            State::InvalidQuery(msg) => Text::styled(
                msg,
                Style::default().fg(self.config.ui.colors.error_fg).bg(self
                    .config
                    .ui
                    .colors
                    .error_bg),
            ),
        };

        let counter = self
            .database
            .as_ref()
            .map(|db| format!("{} / {}", self.hits.len(), db.num_entries()))
            .unwrap_or_else(|| "".to_string());

        let chunks = Layout::default()
            .constraints([
                Constraint::Min(1),
                Constraint::Length(counter.len() as u16 + 1),
            ])
            .direction(Direction::Horizontal)
            .split(area);

        let message = vec![message];
        let message = Paragraph::new(message.iter());
        f.render_widget(message, chunks[0]);

        let counter = vec![Text::raw(counter)];
        let counter = Paragraph::new(counter.iter()).alignment(Alignment::Right);
        f.render_widget(counter, chunks[1]);
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

    fn display_datetime(&self, time: Option<SystemTime>) -> Option<String> {
        time.map(|t| {
            let datetime = DateTime::<Local>::from(t);
            format!("{}", datetime.format(&self.config.ui.datetime_format))
        })
    }

    fn handle_input(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Key(key) => self.handle_key(key)?,
            Event::Mouse(mouse) => self.handle_mouse(mouse)?,
            Event::Resize(_, _) => (),
        };

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        match (key.modifiers, key.code) {
            (_, KeyCode::Esc)
            | (KeyModifiers::CONTROL, KeyCode::Char('c'))
            | (KeyModifiers::CONTROL, KeyCode::Char('g')) => self.status = State::Aborted,
            (_, KeyCode::Enter) => self.status = State::Accepted,
            (_, KeyCode::Up) | (KeyModifiers::CONTROL, KeyCode::Char('p')) => self.on_up()?,
            (_, KeyCode::Down) | (KeyModifiers::CONTROL, KeyCode::Char('n')) => self.on_down()?,
            (_, KeyCode::PageUp) => self.on_pageup()?,
            (_, KeyCode::PageDown) => self.on_pagedown()?,
            (KeyModifiers::CONTROL, KeyCode::Home) | (KeyModifiers::SHIFT, KeyCode::Home) => {
                self.on_scroll_to_top()?;
            }
            (KeyModifiers::CONTROL, KeyCode::End) | (KeyModifiers::SHIFT, KeyCode::End) => {
                self.on_scroll_to_bottom()?;
            }
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
            (_, KeyCode::Char(c)) => {
                self.text_box_state.on_char(c);
                self.on_query_change()?;
            }
            _ => (),
        };

        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> Result<()> {
        match mouse {
            MouseEvent::ScrollUp(_, _, _) => self.on_up()?,
            MouseEvent::ScrollDown(_, _, _) => self.on_down()?,
            _ => (),
        };

        Ok(())
    }

    fn handle_search_result(&mut self, hits: Vec<EntryId>) -> Result<()> {
        self.hits = hits;
        self.status = State::Ready;

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

    fn on_scroll_to_top(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.table_state.select(0);
        }

        Ok(())
    }

    fn on_scroll_to_bottom(&mut self) -> Result<()> {
        if !self.hits.is_empty() {
            self.table_state.select(self.hits.len() - 1);
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
                    .entry(*id)
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
        let query = QueryBuilder::new(query)
            .match_path_mode(self.config.flags.match_path_mode())
            .case_sensitivity(self.config.flags.case_sensitivity())
            .regex(self.config.flags.regex)
            .sort_by(self.config.ui.sort_by)
            .sort_order(self.config.ui.sort_order)
            .sort_dirs_before_files(self.config.ui.sort_dirs_before_files)
            .build();

        match query {
            Ok(query) => {
                self.query = Some(query.clone());
                self.status = State::Searching;
                self.query_tx.as_ref().unwrap().send(query)?;
            }
            Err(err) => {
                // HACK: extract last line to fit in status bar
                self.status = State::InvalidQuery(
                    err.to_string()
                        .split('\n')
                        .map(|s| s.trim())
                        .last()
                        .unwrap_or(&"")
                        .to_string(),
                );
            }
        }

        Ok(())
    }
}

fn setup_terminal() -> Result<Terminal<Backend>> {
    terminal::enable_raw_mode()?;
    let mut stderr = io::stderr();
    crossterm::execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stderr);
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
