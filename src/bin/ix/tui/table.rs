// The MIT License (MIT)
//
// Copyright (c) 2016 Florian Dehau
// Copyright (c) 2020-present mosm
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

// originally from https://github.com/fdehau/tui-rs/blob/72511867624c9bc416e64a1b856026ced5c4e1eb/src/widgets/table.rs

use cassowary::{
    strength::{MEDIUM, REQUIRED, WEAK},
    Expression, Solver,
    WeightedRelation::*,
};
use itertools::izip;
use std::{
    collections::HashMap,
    fmt::Display,
    iter::{self, Iterator},
    ops::Range,
};
use tui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Rect},
    style::Style,
    text::{Span, Spans},
    widgets::{Block, Paragraph, StatefulWidget, Widget},
};
use unicode_width::UnicodeWidthStr;

#[derive(Default, Debug, Clone)]
pub struct TableState {
    offset: usize,
    selected: usize,
}

impl TableState {
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index;
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

impl<M> Default for HighlightableText<M>
where
    M: Iterator<Item = Range<usize>>,
{
    fn default() -> Self {
        Self::Raw(String::new())
    }
}

impl<M> From<String> for HighlightableText<M>
where
    M: Iterator<Item = Range<usize>>,
{
    fn from(s: String) -> Self {
        Self::Raw(s)
    }
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
pub struct Table<'a, H, R, F> {
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
    display_func: F,
}

impl<'a, H, R, M, D, F, T> Table<'a, H, R, F>
where
    H: Iterator,
    H::Item: Display,
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
    R: ExactSizeIterator<Item = T>,
    F: Fn(T) -> Row<M, D>,
{
    pub fn new(header: H, rows: R, display_func: F) -> Table<'a, H, R, F> {
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
            display_func,
        }
    }

    #[allow(dead_code)]
    pub fn block(mut self, block: Block<'a>) -> Table<'a, H, R, F> {
        self.block = Some(block);
        self
    }

    #[allow(dead_code)]
    pub fn header<II>(mut self, header: II) -> Table<'a, H, R, F>
    where
        II: IntoIterator<Item = H::Item, IntoIter = H>,
    {
        self.header = header.into_iter();
        self
    }

    #[allow(dead_code)]
    pub fn header_style(mut self, style: Style) -> Table<'a, H, R, F> {
        self.header_style = style;
        self
    }

    pub fn widths(mut self, widths: &'a [Constraint]) -> Table<'a, H, R, F> {
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

    pub fn alignments(mut self, alignments: &'a [Alignment]) -> Table<'a, H, R, F> {
        self.alignments = Some(alignments);
        self
    }

    #[allow(dead_code)]
    pub fn rows<II>(mut self, rows: II) -> Table<'a, H, R, F>
    where
        II: IntoIterator<Item = T, IntoIter = R>,
    {
        self.rows = rows.into_iter();
        self
    }

    #[allow(dead_code)]
    pub fn style(mut self, style: Style) -> Table<'a, H, R, F> {
        self.style = style;
        self
    }

    pub fn selected_symbol(mut self, selected_symbol: &'a str) -> Table<'a, H, R, F> {
        self.selected_symbol = Some(selected_symbol);
        self
    }

    pub fn selected_style(mut self, selected_style: Style) -> Table<'a, H, R, F> {
        self.selected_style = selected_style;
        self
    }

    pub fn highlight_style(mut self, highlight_style: Style) -> Table<'a, H, R, F> {
        self.highlight_style = highlight_style;
        self
    }

    pub fn selected_highlight_style(
        mut self,
        selected_highlight_style: Style,
    ) -> Table<'a, H, R, F> {
        self.selected_highlight_style = selected_highlight_style;
        self
    }

    pub fn column_spacing(mut self, spacing: u16) -> Table<'a, H, R, F> {
        self.column_spacing = spacing;
        self
    }

    pub fn header_gap(mut self, gap: u16) -> Table<'a, H, R, F> {
        self.header_gap = gap;
        self
    }
}

impl<'a, H, R, M, D, F, T> StatefulWidget for Table<'a, H, R, F>
where
    H: Iterator,
    H::Item: Display,
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
    R: ExactSizeIterator<Item = T>,
    F: Fn(T) -> Row<M, D>,
{
    type State = TableState;

    fn render(mut self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        buf.set_style(area, self.style);
        let table_area = match self.block.take() {
            Some(b) => {
                let inner_area = b.inner(area);
                b.render(area, buf);
                inner_area
            }
            None => area,
        };

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
                .take(self.widths.len())
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
                let text = Span::styled(t.to_string(), self.header_style);
                Paragraph::new(text).alignment(alignment).render(area, buf);

                x += *w + self.column_spacing;
            }
        }
        y += 1 + self.header_gap;

        let selected_symbol = self.selected_symbol.unwrap_or("");
        let blank_symbol = " ".repeat(selected_symbol.width());

        // Draw rows
        let default_style = Style::default();
        if y < table_area.bottom() {
            let remaining = (table_area.bottom() - y) as usize;

            state.offset = state.offset.min(self.rows.len().saturating_sub(remaining));
            state.offset = if state.selected >= remaining + state.offset - 1 {
                state.selected + 1 - remaining
            } else if state.selected < state.offset {
                state.selected
            } else {
                state.offset
            };

            for (i, row) in self
                .rows
                .skip(state.offset)
                .take(remaining)
                .map(self.display_func)
                .enumerate()
            {
                let (style, highlight_style, symbol) = {
                    if i == state.selected - state.offset {
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
                            let text = Span::styled(&text, style);
                            Paragraph::new(text).alignment(alignment).render(area, buf);
                        }
                        HighlightableText::Highlighted(text, ranges) => {
                            let text = build_spans(&text, ranges, &style, &highlight_style);
                            Paragraph::new(text).alignment(alignment).render(area, buf);
                        }
                    }

                    x += width + self.column_spacing;
                }
            }
        }
    }
}

impl<'a, H, R, M, D, F, T> Widget for Table<'a, H, R, F>
where
    H: Iterator,
    H::Item: Display,
    M: Iterator<Item = Range<usize>>,
    D: Iterator<Item = HighlightableText<M>>,
    R: ExactSizeIterator<Item = T>,
    F: Fn(T) -> Row<M, D>,
{
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut state = TableState::default();
        StatefulWidget::render(self, area, buf, &mut state);
    }
}

fn build_spans<'t, M>(
    text: &'t str,
    matches: M,
    style: &Style,
    highlight_style: &Style,
) -> Spans<'t>
where
    M: Iterator<Item = Range<usize>>,
{
    let mut prev_end = 0;
    let mut texts = Vec::new();
    for m in matches {
        if m.start > prev_end {
            texts.push(Span::styled(&text[prev_end..m.start], *style));
        }
        if m.end > m.start {
            texts.push(Span::styled(&text[m.start..m.end], *highlight_style));
        }
        prev_end = m.end;
    }
    if prev_end < text.len() {
        texts.push(Span::styled(&text[prev_end..], *style));
    }
    Spans::from(texts)
}
