use crate::Opt;
use anyhow::Result;
use crossbeam::channel;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use indexa::{Database, Hit};
use rayon::prelude::*;
use rayon::{ThreadPool, ThreadPoolBuilder};
use regex::RegexBuilder;
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::thread;
use tui::backend::{self, CrosstermBackend};
use tui::layout::{Constraint, Layout};
use tui::style::{Color, Modifier, Style};
use tui::widgets::{List, ListState, Paragraph, Text};
use tui::Frame;
use tui::Terminal;

static PROMPT: &str = "> ";

pub fn launch(opt: &Opt) -> Result<()> {
    TuiApp::new(opt)?.launch()
}

enum TuiAppEvent<I> {
    Key(I),
}

struct TuiApp<'a> {
    opt: &'a Opt,
    database: Option<Database>,
    pool: ThreadPool,
    pattern: String,
    hits: Vec<Hit>,
    selected: usize,
}

impl<'a> TuiApp<'a> {
    fn new(opt: &'a Opt) -> Result<Self> {
        let pool = ThreadPoolBuilder::new()
            .num_threads(opt.threads.unwrap())
            .build()?;
        let app = TuiApp {
            opt,
            database: None,
            pool,
            pattern: "".to_string(),
            hits: Vec::new(),
            selected: 0,
        };
        Ok(app)
    }

    fn launch(&mut self) -> Result<()> {
        let database = if self.opt.update || !self.opt.database.exists() {
            println!("Updating database");
            let location = &self.opt.location.as_ref().unwrap();
            let database = self.pool.install(|| Database::new(&location))?;
            let mut writer = BufWriter::new(File::create(&self.opt.database)?);
            bincode::serialize_into(&mut writer, &database)?;
            writer.flush()?;
            database
        } else {
            println!("Loading database");
            let reader = BufReader::new(File::open(&self.opt.database)?);
            bincode::deserialize_from(reader)?
        };
        self.database = Some(database);

        println!("Finished");

        terminal::enable_raw_mode()?;

        let mut stdout = io::stdout();

        crossterm::execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        let (tx, rx) = channel::unbounded();
        thread::spawn(move || loop {
            if let Ok(Event::Key(key)) = event::read() {
                tx.send(TuiAppEvent::Key(key)).unwrap();
            }
        });

        terminal.clear()?;

        loop {
            terminal.hide_cursor()?;
            terminal.draw(|mut f| self.draw(&mut f))?;
            let height = terminal.get_frame().size().height;
            terminal.set_cursor((PROMPT.len() + self.pattern.len()) as u16, height - 1)?;
            terminal.show_cursor()?;

            match rx.recv()? {
                TuiAppEvent::Key(key) => match key {
                    KeyEvent {
                        code: KeyCode::Esc, ..
                    }
                    | KeyEvent {
                        code: KeyCode::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                    } => {
                        terminal::disable_raw_mode()?;
                        crossterm::execute!(
                            terminal.backend_mut(),
                            LeaveAlternateScreen,
                            DisableMouseCapture
                        )?;
                        break;
                    }
                    KeyEvent {
                        code: KeyCode::Char(c),
                        ..
                    } => {
                        self.pattern.push(c);
                        self.on_pattern_change()?;
                    }
                    KeyEvent {
                        code: KeyCode::Backspace,
                        ..
                    } => {
                        self.pattern.pop();
                        self.on_pattern_change()?;
                    }
                    KeyEvent {
                        code: KeyCode::Up, ..
                    } => self.on_up()?,
                    KeyEvent {
                        code: KeyCode::Down,
                        ..
                    } => self.on_down()?,
                    KeyEvent {
                        code: KeyCode::Enter,
                        ..
                    } => {
                        terminal::disable_raw_mode()?;
                        crossterm::execute!(
                            terminal.backend_mut(),
                            LeaveAlternateScreen,
                            DisableMouseCapture
                        )?;

                        if let Some(hit) = self.hits.get(self.selected) {
                            println!(
                                "{}",
                                self.database
                                    .as_ref()
                                    .unwrap()
                                    .path_from_hit(hit)
                                    .to_str()
                                    .ok_or(indexa::Error::Utf8)?
                            );
                        }

                        break;
                    }
                    _ => {}
                },
            }
        }

        Ok(())
    }

    fn draw<B: backend::Backend>(&self, f: &mut Frame<B>) {
        let chunks = Layout::default()
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(f.size());

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

        let text = [
            Text::styled(
                PROMPT,
                Style::default().fg(Color::Green).modifier(Modifier::BOLD),
            ),
            Text::raw(&self.pattern),
        ];
        let paragraph = Paragraph::new(text.iter());
        f.render_widget(paragraph, chunks[1]);
    }

    fn on_pattern_change(&mut self) -> Result<()> {
        if self.pattern.is_empty() {
            self.hits.clear();
            return Ok(());
        }

        let pattern = if self.opt.regex {
            RegexBuilder::new(&self.pattern)
        } else {
            RegexBuilder::new(&regex::escape(&self.pattern))
        }
        .case_insensitive(!self.opt.case_sensitive)
        .build()?;

        self.hits = self.pool.install(|| {
            let mut hits = self
                .database
                .as_ref()
                .unwrap()
                .search(&pattern, self.opt.in_path);
            hits.as_parallel_slice_mut()
                .par_sort_unstable_by_key(|hit| {
                    self.database.as_ref().unwrap().status_from_hit(hit).mtime
                });
            hits
        });

        self.selected = self.selected.min(self.hits.len().saturating_sub(1));

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
}
