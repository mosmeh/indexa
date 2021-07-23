use crossterm::{
    cursor::MoveTo,
    queue,
    style::{
        Attribute as CAttribute, Color as CColor, Print, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
};
use std::io::{self, Write};
use tui::{
    backend::{Backend, CrosstermBackend},
    buffer::Cell,
    layout::Rect,
    style::{Color, Modifier},
};

pub struct CustomBackend<W: Write>(CrosstermBackend<W>);

impl<W> CustomBackend<W>
where
    W: Write,
{
    pub fn new(buffer: W) -> CustomBackend<W> {
        Self(CrosstermBackend::new(buffer))
    }
}

impl<W> Write for CustomBackend<W>
where
    W: Write,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Write::flush(&mut self.0)
    }
}

impl<W> Backend for CustomBackend<W>
where
    W: Write,
{
    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut buffer = Vec::with_capacity(content.size_hint().0 * 3);
        let mut fg = Color::Reset;
        let mut bg = Color::Reset;
        let mut modifier = Modifier::empty();
        let mut last_pos: Option<(u16, u16)> = None;
        for (x, y, cell) in content {
            // Move the cursor if the previous location was not (x - 1, y)
            if !matches!(last_pos, Some(p) if x == p.0 + 1 && y == p.1) {
                map_error(queue!(buffer, MoveTo(x, y)))?;
            }
            last_pos = Some((x, y));
            if cell.modifier != modifier {
                let diff = ModifierDiff {
                    from: modifier,
                    to: cell.modifier,
                };
                diff.queue(&mut buffer)?;
                modifier = cell.modifier;
            }
            if cell.fg != fg {
                let color = CColorWrapper::from(cell.fg).0;
                map_error(queue!(buffer, SetForegroundColor(color)))?;
                fg = cell.fg;
            }
            if cell.bg != bg {
                let color = CColorWrapper::from(cell.bg).0;
                map_error(queue!(buffer, SetBackgroundColor(color)))?;
                bg = cell.bg;
            }

            map_error(queue!(buffer, Print(&cell.symbol)))?;
        }

        let string = std::str::from_utf8(&buffer).unwrap();
        map_error(queue!(
            self.0,
            Print(string),
            SetForegroundColor(CColor::Reset),
            SetBackgroundColor(CColor::Reset),
            SetAttribute(CAttribute::Reset)
        ))
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.0.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.0.show_cursor()
    }

    fn get_cursor(&mut self) -> io::Result<(u16, u16)> {
        self.0.get_cursor()
    }

    fn set_cursor(&mut self, x: u16, y: u16) -> io::Result<()> {
        self.0.set_cursor(x, y)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.0.clear()
    }

    fn size(&self) -> io::Result<Rect> {
        self.0.size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.0)
    }
}

fn map_error(error: crossterm::Result<()>) -> io::Result<()> {
    error.map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
}

struct CColorWrapper(CColor);

impl From<Color> for CColorWrapper {
    fn from(color: Color) -> Self {
        let c = match color {
            Color::Reset => CColor::Reset,
            Color::Black => CColor::Black,
            Color::Red => CColor::DarkRed,
            Color::Green => CColor::DarkGreen,
            Color::Yellow => CColor::DarkYellow,
            Color::Blue => CColor::DarkBlue,
            Color::Magenta => CColor::DarkMagenta,
            Color::Cyan => CColor::DarkCyan,
            Color::Gray => CColor::Grey,
            Color::DarkGray => CColor::DarkGrey,
            Color::LightRed => CColor::Red,
            Color::LightGreen => CColor::Green,
            Color::LightBlue => CColor::Blue,
            Color::LightYellow => CColor::Yellow,
            Color::LightMagenta => CColor::Magenta,
            Color::LightCyan => CColor::Cyan,
            Color::White => CColor::White,
            Color::Indexed(i) => CColor::AnsiValue(i),
            Color::Rgb(r, g, b) => CColor::Rgb { r, g, b },
        };
        Self(c)
    }
}

#[derive(Debug)]
struct ModifierDiff {
    pub from: Modifier,
    pub to: Modifier,
}

impl ModifierDiff {
    fn queue<W>(&self, mut w: W) -> io::Result<()>
    where
        W: io::Write,
    {
        //use crossterm::Attribute;
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            map_error(queue!(w, SetAttribute(CAttribute::NoReverse)))?;
        }
        if removed.contains(Modifier::BOLD) {
            map_error(queue!(w, SetAttribute(CAttribute::NormalIntensity)))?;
            if self.to.contains(Modifier::DIM) {
                map_error(queue!(w, SetAttribute(CAttribute::Dim)))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            map_error(queue!(w, SetAttribute(CAttribute::NoItalic)))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            map_error(queue!(w, SetAttribute(CAttribute::NoUnderline)))?;
        }
        if removed.contains(Modifier::DIM) {
            map_error(queue!(w, SetAttribute(CAttribute::NormalIntensity)))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            map_error(queue!(w, SetAttribute(CAttribute::NotCrossedOut)))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            map_error(queue!(w, SetAttribute(CAttribute::NoBlink)))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            map_error(queue!(w, SetAttribute(CAttribute::Reverse)))?;
        }
        if added.contains(Modifier::BOLD) {
            map_error(queue!(w, SetAttribute(CAttribute::Bold)))?;
        }
        if added.contains(Modifier::ITALIC) {
            map_error(queue!(w, SetAttribute(CAttribute::Italic)))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            map_error(queue!(w, SetAttribute(CAttribute::Underlined)))?;
        }
        if added.contains(Modifier::DIM) {
            map_error(queue!(w, SetAttribute(CAttribute::Dim)))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            map_error(queue!(w, SetAttribute(CAttribute::CrossedOut)))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            map_error(queue!(w, SetAttribute(CAttribute::SlowBlink)))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            map_error(queue!(w, SetAttribute(CAttribute::RapidBlink)))?;
        }

        Ok(())
    }
}
