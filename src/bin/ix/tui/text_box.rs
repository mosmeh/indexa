use tui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Span, Spans},
    widgets::{Paragraph, StatefulWidget, Widget},
};
use unicode_segmentation::{GraphemeCursor, UnicodeSegmentation};

pub struct TextBox<'b> {
    style: Style,
    highlight_style: Style,
    prompt: Span<'b>,
}

impl<'b> TextBox<'b> {
    pub fn new() -> Self {
        Self {
            style: Default::default(),
            highlight_style: Default::default(),
            prompt: Span::raw(""),
        }
    }

    #[allow(dead_code)]
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }

    pub fn prompt(mut self, prompt: Span<'b>) -> Self {
        self.prompt = prompt;
        self
    }
}

impl StatefulWidget for TextBox<'_> {
    type State = TextBoxState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let grapheme_indices = UnicodeSegmentation::grapheme_indices(state.text.as_str(), true);
        let mut text = vec![self.prompt.clone()];
        text.extend(grapheme_indices.map(|(i, grapheme)| {
            if i == state.grapheme_cursor.cur_cursor() {
                Span::styled(grapheme, self.highlight_style)
            } else {
                Span::styled(grapheme, self.style)
            }
        }));
        if state.grapheme_cursor.cur_cursor() >= state.text.len() {
            text.push(Span::styled(" ", self.highlight_style));
        }

        let paragraph = Paragraph::new(Spans::from(text));
        paragraph.render(area, buf);
    }
}

pub struct TextBoxState {
    text: String,
    grapheme_cursor: GraphemeCursor,
}

impl TextBoxState {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Default::default()
    }

    pub fn with_text(text: String) -> Self {
        let len = text.len();
        Self {
            text,
            grapheme_cursor: GraphemeCursor::new(len, len, true),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.grapheme_cursor = GraphemeCursor::new(0, 0, true);
    }

    pub fn on_char(&mut self, ch: char) {
        self.text.insert(self.grapheme_cursor.cur_cursor(), ch);
        self.grapheme_cursor =
            GraphemeCursor::new(self.grapheme_cursor.cur_cursor(), self.text.len(), true);
        self.grapheme_cursor
            .next_boundary(
                &self.text[self.grapheme_cursor.cur_cursor()..],
                self.grapheme_cursor.cur_cursor(),
            )
            .unwrap();
    }

    pub fn on_backspace(&mut self) -> bool {
        if self.grapheme_cursor.cur_cursor() > 0 {
            self.grapheme_cursor
                .prev_boundary(&self.text[..self.grapheme_cursor.cur_cursor()], 0)
                .unwrap();
            self.text.remove(self.grapheme_cursor.cur_cursor());
            self.grapheme_cursor =
                GraphemeCursor::new(self.grapheme_cursor.cur_cursor(), self.text.len(), true);

            true
        } else {
            false
        }
    }

    pub fn on_delete(&mut self) -> bool {
        if self.grapheme_cursor.cur_cursor() < self.text.len() {
            self.text.remove(self.grapheme_cursor.cur_cursor());
            self.grapheme_cursor =
                GraphemeCursor::new(self.grapheme_cursor.cur_cursor(), self.text.len(), true);

            true
        } else {
            false
        }
    }

    pub fn on_left(&mut self) -> bool {
        let prev_cursor = self.grapheme_cursor.cur_cursor();
        self.grapheme_cursor
            .prev_boundary(&self.text[..self.grapheme_cursor.cur_cursor()], 0)
            .unwrap();
        self.grapheme_cursor.cur_cursor() < prev_cursor
    }

    pub fn on_right(&mut self) -> bool {
        let prev_cursor = self.grapheme_cursor.cur_cursor();
        self.grapheme_cursor
            .next_boundary(
                &self.text[self.grapheme_cursor.cur_cursor()..],
                self.grapheme_cursor.cur_cursor(),
            )
            .unwrap();
        self.grapheme_cursor.cur_cursor() > prev_cursor
    }

    pub fn on_home(&mut self) -> bool {
        if self.grapheme_cursor.cur_cursor() > 0 {
            self.grapheme_cursor = GraphemeCursor::new(0, self.text.len(), true);

            true
        } else {
            false
        }
    }

    pub fn on_end(&mut self) -> bool {
        if self.grapheme_cursor.cur_cursor() < self.text.len() {
            self.grapheme_cursor = GraphemeCursor::new(self.text.len(), self.text.len(), true);

            true
        } else {
            false
        }
    }
}

impl Default for TextBoxState {
    fn default() -> Self {
        Self {
            text: "".to_string(),
            grapheme_cursor: GraphemeCursor::new(0, 0, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit() {
        let mut state = TextBoxState::new();
        assert_eq!("", state.text());
        state.on_char('a');
        assert_eq!("a", state.text());
        state.on_left();
        state.on_char('x');
        assert_eq!("xa", state.text());
        state.on_char('あ');
        assert_eq!("xあa", state.text());
        state.on_backspace();
        assert_eq!("xa", state.text());
        state.on_end();
        state.on_char('亜');
        assert_eq!("xa亜", state.text());
        state.on_left();
        state.on_delete();
        assert_eq!("xa", state.text());
        state.on_home();
        state.on_char('𠮷');
        assert_eq!("𠮷xa", state.text());
        state.on_right();
        state.on_char('b');
        assert_eq!("𠮷xba", state.text());

        let mut state2 = TextBoxState::with_text("𠮷x".to_string());
        state2.on_char('b');
        state2.on_char('a');
        assert_eq!(state.text(), state2.text());

        state.clear();
        assert_eq!("", state.text());
    }
}
