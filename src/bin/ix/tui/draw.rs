use super::{
    table::{HighlightableText, Row, Table},
    text_box::TextBox,
    Backend, State, TuiApp,
};

use indexa::{
    database::{Entry, EntryId, StatusKind},
    mode::Mode,
    query::{Query, SortOrder},
};

use chrono::{offset::Local, DateTime};
use std::{ops::Range, time::SystemTime};
use tui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Span,
    widgets::Paragraph,
    Frame,
};

impl<'a> TuiApp<'a> {
    pub fn draw(&mut self, f: &mut Frame<Backend>, terminal_width: u16) {
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
        let text = Span::raw(
            self.hits
                .get(self.table_state.selected())
                .map(|id| {
                    self.database
                        .as_ref()
                        .unwrap()
                        .entry(*id)
                        .path()
                        .as_str()
                        .to_owned()
                })
                .unwrap_or_default(),
        );
        let paragraph = Paragraph::new(text);
        f.render_widget(paragraph, chunks[2]);

        // input box
        let text_box = TextBox::new()
            .highlight_style(Style::default().fg(Color::Black).bg(Color::White))
            .prompt(Span::styled(
                "> ",
                Style::default()
                    .fg(self.config.ui.colors.prompt)
                    .add_modifier(Modifier::BOLD),
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
                column.status.to_string()
            }
        });

        #[allow(clippy::needless_collect)] // false positive
        let display_func = |id: &EntryId| {
            let entry = self.database.as_ref().unwrap().entry(*id);
            let contents = columns
                .iter()
                .map(|column| {
                    self.format_column_content(&column.status, &entry, self.query.as_ref().unwrap())
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

        self.page_scroll_amount = area
            .height
            .saturating_sub(
                // header
                1 +
                // header_gap
                1 +
                // one less than page height
                1,
            )
            .max(1);
    }

    fn draw_status_bar(&self, f: &mut Frame<Backend>, area: Rect) {
        let message = match &self.status {
            State::Loading => Span::raw("Loading database"),
            State::Searching => Span::raw("Searching"),
            State::Ready | State::Aborted | State::Accepted => Span::raw("Ready"),
            State::InvalidQuery(msg) => Span::styled(
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

        let message = Paragraph::new(message);
        f.render_widget(message, chunks[0]);

        let counter = Span::raw(counter);
        let counter = Paragraph::new(counter).alignment(Alignment::Right);
        f.render_widget(counter, chunks[1]);
    }

    fn format_column_content(
        &self,
        kind: &StatusKind,
        entry: &Entry,
        query: &Query,
    ) -> HighlightableText<impl Iterator<Item = Range<usize>>> {
        match kind {
            StatusKind::Basename => HighlightableText::Highlighted(
                entry.basename().to_owned(),
                query.basename_matches(entry).into_iter(),
            ),
            StatusKind::Path => HighlightableText::Highlighted(
                entry.path().as_str().to_owned(),
                query.path_matches(entry).into_iter(),
            ),
            StatusKind::Extension => entry
                .extension()
                .map(|s| s.to_string().into())
                .unwrap_or_default(),
            StatusKind::Size => entry
                .size()
                .map(|size| self.format_size(size, entry.is_dir()).into())
                .unwrap_or_default(),
            StatusKind::Mode => entry
                .mode()
                .map(|mode| self.format_mode(mode).into())
                .unwrap_or_default(),
            StatusKind::Created => entry
                .created()
                .map(|created| self.format_datetime(created).into())
                .unwrap_or_default(),
            StatusKind::Modified => entry
                .modified()
                .map(|modified| self.format_datetime(modified).into())
                .unwrap_or_default(),
            StatusKind::Accessed => entry
                .accessed()
                .map(|accessed| self.format_datetime(accessed).into())
                .unwrap_or_default(),
        }
    }

    fn format_size(&self, size: u64, is_dir: bool) -> String {
        if is_dir {
            if size == 1 {
                format!("{} item", size)
            } else {
                format!("{} items", size)
            }
        } else if self.config.ui.human_readable_size {
            size::Size::Bytes(size).to_string(size::Base::Base2, size::Style::Abbreviated)
        } else {
            size.to_string()
        }
    }

    fn format_mode(&self, mode: Mode) -> String {
        #[cfg(unix)]
        {
            use crate::config::ModeFormatUnix;

            match self.config.ui.unix.mode_format {
                ModeFormatUnix::Octal => mode.display_octal().to_string(),
                ModeFormatUnix::Symbolic => mode.display_symbolic().to_string(),
            }
        }

        #[cfg(windows)]
        {
            use crate::config::ModeFormatWindows;

            match self.config.ui.windows.mode_format {
                ModeFormatWindows::Traditional => mode.display_traditional().to_string(),
                ModeFormatWindows::PowerShell => mode.display_powershell().to_string(),
            }
        }
    }

    fn format_datetime(&self, time: SystemTime) -> String {
        let datetime = DateTime::<Local>::from(time);
        datetime.format(&self.config.ui.datetime_format).to_string()
    }
}
