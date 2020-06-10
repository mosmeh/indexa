// Most of this file was copied and pasted from tui-rs
// https://github.com/fdehau/tui-rs/blob/72511867624c9bc416e64a1b856026ced5c4e1eb/src/widgets/table.rs

use cassowary::strength::{MEDIUM, REQUIRED, WEAK};
use cassowary::WeightedRelation::*;
use cassowary::{Expression, Solver};
use itertools::izip;
use std::collections::HashMap;
use std::fmt::Display;
use std::iter::{self, Iterator};
use std::ops::Range;
use tui::buffer::Buffer;
use tui::layout::{Alignment, Constraint, Rect};
use tui::style::Style;
use tui::widgets::{Block, Paragraph, StatefulWidget, Text, Widget};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
pub struct TableState {
    offset: usize,
    selected: Option<usize>,
}

impl Default for TableState {
    fn default() -> TableState {
        TableState {
            offset: 0,
            selected: None,
        }
    }
}

impl TableState {
    #[allow(dead_code)]
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index;
        if index.is_none() {
            self.offset = 0;
        }
    }
}

#[derive(Debug, Clone)]
pub enum HighlightableText<M>
where
    M: Iterator<Item = Range<usize>>,
{
    Raw(String),
    Highlighted(String, M),
}

#[derive(Debug, Clone)]
pub struct Row<M, D>
where
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
{
    data: D,
}

impl<M, D> Row<M, D>
where
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
{
    pub fn new(data: D) -> Self {
        Self { data }
    }
}

#[derive(Debug, Clone)]
pub struct Table<'a, H, R> {
    block: Option<Block<'a>>,
    style: Style,
    header: H,
    header_style: Style,
    widths: &'a [Constraint],
    alignments: Option<&'a [Alignment]>,
    column_spacing: u16,
    header_gap: u16,
    selected_style: Style,
    highlight_style: Style,
    selected_highlight_style: Style,
    selected_symbol: Option<&'a str>,
    rows: R,
}

impl<'a, H, R> Default for Table<'a, H, R>
where
    H: Iterator + Default,
    R: Iterator + Default,
{
    fn default() -> Table<'a, H, R> {
        Table {
            block: None,
            style: Style::default(),
            header: H::default(),
            header_style: Style::default(),
            widths: &[],
            alignments: None,
            column_spacing: 1,
            header_gap: 1,
            selected_style: Style::default(),
            highlight_style: Style::default(),
            selected_highlight_style: Style::default(),
            selected_symbol: None,
            rows: R::default(),
        }
    }
}
impl<'a, H, R, M, D> Table<'a, H, R>
where
    H: Iterator,
    H::Item: Display,
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
    R: Iterator<Item = Row<M, D>>,
{
    pub fn new(header: H, rows: R) -> Table<'a, H, R> {
        Table {
            block: None,
            style: Style::default(),
            header,
            header_style: Style::default(),
            widths: &[],
            alignments: None,
            column_spacing: 1,
            header_gap: 1,
            selected_style: Style::default(),
            highlight_style: Style::default(),
            selected_highlight_style: Style::default(),
            selected_symbol: None,
            rows,
        }
    }

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> Table<'a, H, R> {
        self.block = Some(block);
        self
    }

    #[allow(dead_code)]
    pub fn header<II>(mut self, header: II) -> Table<'a, H, R>
    where
        II: IntoIterator<Item = H::Item, IntoIter = H>,
    {
        self.header = header.into_iter();
        self
    }

    #[allow(dead_code)]
    pub fn header_style(mut self, style: Style) -> Table<'a, H, R> {
        self.header_style = style;
        self
    }

    pub fn widths(mut self, widths: &'a [Constraint]) -> Table<'a, H, R> {
        let between_0_and_100 = |&w| match w {
            Constraint::Percentage(p) => p <= 100,
            _ => true,
        };
        assert!(
            widths.iter().all(between_0_and_100),
            "Percentages should be between 0 and 100 inclusively."
        );
        self.widths = widths;
        self
    }

    pub fn alignments(mut self, alignments: &'a [Alignment]) -> Table<'a, H, R> {
        self.alignments = Some(alignments);
        self
    }

    #[allow(dead_code)]
    pub fn rows<II>(mut self, rows: II) -> Table<'a, H, R>
    where
        II: IntoIterator<Item = Row<M, D>, IntoIter = R>,
    {
        self.rows = rows.into_iter();
        self
    }

    #[allow(dead_code)]
    pub fn style(mut self, style: Style) -> Table<'a, H, R> {
        self.style = style;
        self
    }

    pub fn selected_symbol(mut self, selected_symbol: &'a str) -> Table<'a, H, R> {
        self.selected_symbol = Some(selected_symbol);
        self
    }

    pub fn selected_style(mut self, selected_style: Style) -> Table<'a, H, R> {
        self.selected_style = selected_style;
        self
    }

    pub fn highlight_style(mut self, highlight_style: Style) -> Table<'a, H, R> {
        self.highlight_style = highlight_style;
        self
    }

    pub fn selected_highlight_style(mut self, selected_highlight_style: Style) -> Table<'a, H, R> {
        self.selected_highlight_style = selected_highlight_style;
        self
    }

    #[allow(dead_code)]
    pub fn column_spacing(mut self, spacing: u16) -> Table<'a, H, R> {
        self.column_spacing = spacing;
        self
    }

    #[allow(dead_code)]
    pub fn header_gap(mut self, gap: u16) -> Table<'a, H, R> {
        self.header_gap = gap;
        self
    }
}

impl<'a, 'b, H, R, M, D> StatefulWidget for Table<'a, H, R>
where
    H: Iterator,
    H::Item: Display,
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
    R: Iterator<Item = Row<M, D>>,
{
    type State = TableState;

    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        // Render block if necessary and get the drawing area
        let table_area = match self.block {
            Some(ref mut b) => {
                b.render(area, buf);
                b.inner(area)
            }
            None => area,
        };

        buf.set_background(table_area, self.style.bg);

        let mut solver = Solver::new();
        let mut var_indices = HashMap::new();
        let mut ccs = Vec::new();
        let mut variables = Vec::new();
        for i in 0..self.widths.len() {
            let var = cassowary::Variable::new();
            variables.push(var);
            var_indices.insert(var, i);
        }
        for (i, constraint) in self.widths.iter().enumerate() {
            ccs.push(variables[i] | GE(WEAK) | 0.);
            ccs.push(match *constraint {
                Constraint::Length(v) => variables[i] | EQ(MEDIUM) | f64::from(v),
                Constraint::Percentage(v) => {
                    variables[i] | EQ(WEAK) | (f64::from(v * area.width) / 100.0)
                }
                Constraint::Ratio(n, d) => {
                    variables[i] | EQ(WEAK) | (f64::from(area.width) * f64::from(n) / f64::from(d))
                }
                Constraint::Min(v) => variables[i] | GE(WEAK) | f64::from(v),
                Constraint::Max(v) => variables[i] | LE(WEAK) | f64::from(v),
            })
        }
        solver
            .add_constraint(
                variables
                    .iter()
                    .fold(Expression::from_constant(0.), |acc, v| acc + *v)
                    | LE(REQUIRED)
                    | f64::from(
                        area.width - 2 - (self.column_spacing * (variables.len() as u16 - 1)),
                    ),
            )
            .unwrap();
        solver.add_constraints(&ccs).unwrap();
        let mut solved_widths = vec![0; variables.len()];
        for &(var, value) in solver.fetch_changes() {
            let index = var_indices[&var];
            let value = if value.is_sign_negative() {
                0
            } else {
                value as u16
            };
            solved_widths[index] = value
        }

        let alignments: Vec<_> = if let Some(alignments) = self.alignments {
            alignments.iter().collect()
        } else {
            iter::repeat(&Alignment::Left)
                .take(self.widths.iter().count())
                .collect()
        };

        let mut y = table_area.top();
        let mut x = table_area.left();

        // Draw header
        if y < table_area.bottom() {
            for (w, &&alignment, t) in izip!(
                solved_widths.iter(),
                alignments.iter(),
                self.header.by_ref(),
            ) {
                let area = Rect {
                    x,
                    y,
                    width: *w,
                    height: 1,
                };
                let text = Text::styled(format!("{}", t), self.header_style);
                Paragraph::new(vec![&text].into_iter())
                    .alignment(alignment)
                    .render(area, buf);

                x += *w + self.column_spacing;
            }
        }
        y += 1 + self.header_gap;

        let selected_symbol = self.selected_symbol.unwrap_or("");
        let blank_symbol = iter::repeat(" ")
            .take(selected_symbol.width())
            .collect::<String>();

        // Draw rows
        let default_style = Style::default();
        if y < table_area.bottom() {
            let remaining = (table_area.bottom() - y) as usize;

            // Make sure the table shows the selected item
            state.offset = if let Some(selected) = state.selected {
                if selected >= remaining + state.offset - 1 {
                    selected + 1 - remaining
                } else if selected < state.offset {
                    selected
                } else {
                    state.offset
                }
            } else {
                0
            };
            for (i, row) in self.rows.skip(state.offset).take(remaining).enumerate() {
                let (style, highlight_style, symbol) = {
                    if Some(i) == state.selected.map(|s| s - state.offset) {
                        (
                            self.selected_style,
                            self.selected_highlight_style,
                            selected_symbol,
                        )
                    } else {
                        (default_style, self.highlight_style, blank_symbol.as_ref())
                    }
                };

                x = table_area.left();

                buf.set_stringn(x, y + i as u16, &symbol, symbol.width(), style);
                x += symbol.width() as u16;

                for (c, (w, &&alignment, elt)) in
                    izip!(solved_widths.iter(), alignments.iter(), row.data).enumerate()
                {
                    let width = if c == 0 {
                        *w - symbol.width() as u16
                    } else {
                        *w
                    };
                    let area = Rect {
                        x,
                        y: y + i as u16,
                        width,
                        height: 1,
                    };

                    match elt {
                        HighlightableText::Raw(text) => {
                            let text = Text::styled(&text, style);
                            Paragraph::new(vec![&text].into_iter())
                                .alignment(alignment)
                                .render(area, buf);
                        }
                        HighlightableText::Highlighted(text, ranges) => {
                            let texts = build_texts(&text, ranges, &style, &highlight_style);
                            Paragraph::new(texts.iter())
                                .alignment(alignment)
                                .render(area, buf);
                        }
                    }

                    x += width + self.column_spacing;
                }
            }
        }
    }
}

impl<'a, H, R, M, D> Widget for Table<'a, H, R>
where
    H: Iterator,
    H::Item: Display,
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
    R: Iterator<Item = Row<M, D>>,
{
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut state = TableState::default();
        StatefulWidget::render(self, area, buf, &mut state);
    }
}

fn build_texts<'t, M>(
    text: &'t str,
    matches: M,
    style: &Style,
    highlight_style: &Style,
) -> Vec<Text<'t>>
where
    M: Iterator<Item = Range<usize>>,
{
    let mut prev_end = 0;
    let mut texts = Vec::new();
    for m in matches {
        if m.start > prev_end {
            texts.push(Text::styled(&text[prev_end..m.start], *style));
        }
        if m.end > m.start {
            texts.push(Text::styled(&text[m.start..m.end], *highlight_style));
        }
        prev_end = m.end;
    }
    if prev_end < text.len() {
        texts.push(Text::styled(&text[prev_end..], *style));
    }
    texts
}
