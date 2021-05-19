use super::{State, TuiApp};

use indexa::{database::EntryId, query::QueryBuilder};

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

impl<'a> TuiApp<'a> {
    pub fn handle_input(&mut self, event: Event) -> Result<()> {
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
        match mouse.kind {
            MouseEventKind::ScrollUp => self.on_up()?,
            MouseEventKind::ScrollDown => self.on_down()?,
            _ => (),
        };

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

    pub fn handle_search_result(&mut self, hits: Vec<EntryId>) -> Result<()> {
        self.hits = hits;
        self.status = State::Ready;

        if !self.hits.is_empty() {
            self.table_state
                .select(self.table_state.selected().min(self.hits.len() - 1));
        }

        Ok(())
    }

    pub fn on_accept(&self) -> Result<()> {
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

    pub fn on_query_change(&mut self) -> Result<()> {
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
