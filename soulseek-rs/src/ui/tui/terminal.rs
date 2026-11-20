//! The terminal backend and frame the widgets draw into.

use super::buffer::Buffer;
use super::layout::{Position, Rect, Size};
use super::widgets::{StatefulWidget, Widget};
use rustix::termios::{self, OptionalActions, Termios};
use std::fmt;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

const ENTER_SCREEN: &[u8] = b"\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H\x1b[?1000h\x1b[?1002h\x1b[?1003h\x1b[?1006h";
const LEAVE_SCREEN: &[u8] = b"\x1b[0m\x1b[?25h\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?1049l";

static ORIGINAL_TERMIOS: Mutex<Option<Termios>> = Mutex::new(None);
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_RESTORE_INSTALLED: OnceLock<()> = OnceLock::new();

pub trait Backend {
    fn size(&mut self) -> io::Result<Size>;
    fn draw(&mut self, previous: &Buffer, current: &Buffer) -> io::Result<()>;
    fn set_cursor(&mut self, position: Option<Position>) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
}

pub struct TestBackend {
    buffer: Buffer,
    cursor: Option<Position>,
    cursor_visible: bool,
}

impl TestBackend {
    #[must_use]
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            buffer: Buffer::empty(Rect::new(0, 0, width, height)),
            cursor: None,
            cursor_visible: false,
        }
    }

    #[must_use]
    pub const fn buffer(&self) -> &Buffer {
        &self.buffer
    }
}

impl Backend for TestBackend {
    fn size(&mut self) -> io::Result<Size> {
        Ok(Size::new(self.buffer.area.width, self.buffer.area.height))
    }

    fn draw(&mut self, _previous: &Buffer, current: &Buffer) -> io::Result<()> {
        self.buffer = Buffer {
            area: current.area,
            content: current.content.clone(),
        };
        Ok(())
    }

    fn set_cursor(&mut self, position: Option<Position>) -> io::Result<()> {
        self.cursor = position;
        self.cursor_visible = position.is_some();
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl fmt::Display for TestBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.buffer.to_text())
    }
}

pub struct AnsiBackend;

impl AnsiBackend {
    pub fn enter() -> io::Result<Self> {
        install_panic_restore();
        let stdin = io::stdin();
        let mut original = ORIGINAL_TERMIOS
            .lock()
            .map_err(|_| io::Error::other("terminal state poisoned"))?;
        if original.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "terminal session already active",
            ));
        }
        let saved = termios::tcgetattr(&stdin)?;
        let mut raw = saved.clone();
        raw.make_raw();
        termios::tcsetattr(&stdin, OptionalActions::Now, &raw)?;
        *original = Some(saved);
        TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
        drop(original);
        let mut stdout = io::stdout().lock();
        if let Err(error) = stdout.write_all(ENTER_SCREEN).and_then(|()| stdout.flush()) {
            let _ = restore_terminal();
            return Err(error);
        }
        Ok(Self)
    }
}

impl Drop for AnsiBackend {
    fn drop(&mut self) {
        let _ = restore_terminal();
    }
}

impl Backend for AnsiBackend {
    fn size(&mut self) -> io::Result<Size> {
        let size = termios::tcgetwinsize(io::stdout())?;
        Ok(Size::new(size.ws_col.max(1), size.ws_row.max(1)))
    }

    fn draw(&mut self, previous: &Buffer, current: &Buffer) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(encode_changes(previous, current).as_bytes())
    }

    fn set_cursor(&mut self, position: Option<Position>) -> io::Result<()> {
        let mut stdout = io::stdout().lock();
        match position {
            Some(position) => {
                let mut output = String::new();
                output.push_str("\x1b[?25h");
                write_move(&mut output, position.x, position.y);
                stdout.write_all(output.as_bytes())
            }
            None => stdout.write_all(b"\x1b[?25l"),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stdout().lock().flush()
    }
}

/// The bytes that turn `previous` into `current` on a terminal: cursor moves
/// and style changes around the symbols of the cells that differ.
fn encode_changes(previous: &Buffer, current: &Buffer) -> String {
    let mut output = String::new();
    let mut output_position: Option<(u16, u16)> = None;
    let mut output_style = None;
    for y in current.area.y..current.area.bottom() {
        for x in current.area.x..current.area.right() {
            let Some(cell) = current.cell(Position::new(x, y)) else {
                continue;
            };
            if cell.width == 0 {
                continue;
            }
            let unchanged = previous
                .cell(Position::new(x, y))
                .is_some_and(|old| old == cell);
            if unchanged {
                continue;
            }
            if output_position != Some((x, y)) {
                write_move(&mut output, x, y);
            }
            if output_style != Some(cell.style) {
                output.push_str(&cell.style.sgr());
                output_style = Some(cell.style);
            }
            output.push_str(&cell.symbol);
            output_position = Some((x.saturating_add(cell.width()), y));
        }
    }
    output
}

fn write_move(output: &mut String, x: u16, y: u16) {
    let _ = std::fmt::Write::write_fmt(
        output,
        format_args!("\x1b[{};{}H", y.saturating_add(1), x.saturating_add(1)),
    );
}

pub fn restore_terminal() -> io::Result<()> {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    let stdin = io::stdin();
    let saved = ORIGINAL_TERMIOS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    let mut first_error = saved
        .as_ref()
        .and_then(|saved| termios::tcsetattr(&stdin, OptionalActions::Now, saved).err())
        .map(Into::into);
    let mut stdout = io::stdout().lock();
    if let Err(error) = stdout.write_all(LEAVE_SCREEN).and_then(|()| stdout.flush())
        && first_error.is_none()
    {
        first_error = Some(error);
    }
    first_error.map_or(Ok(()), Err)
}

fn install_panic_restore() {
    PANIC_RESTORE_INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic| {
            let _ = restore_terminal();
            previous(panic);
        }));
    });
}

pub struct Frame<'a> {
    area: Rect,
    buffer: &'a mut Buffer,
    cursor: Option<Position>,
}

impl Frame<'_> {
    #[must_use]
    pub const fn area(&self) -> Rect {
        self.area
    }

    pub fn render_widget<W: Widget>(&mut self, widget: W, area: Rect) {
        widget.render(area, self.buffer);
    }

    pub fn render_stateful_widget<W: StatefulWidget>(
        &mut self,
        widget: W,
        area: Rect,
        state: &mut W::State,
    ) {
        widget.render(area, self.buffer, state);
    }

    pub const fn set_cursor_position(&mut self, position: Position) {
        self.cursor = Some(position);
    }
}

pub struct Terminal<B: Backend> {
    backend: B,
    previous: Buffer,
    size: Size,
}

impl<B: Backend> Terminal<B> {
    pub fn new(mut backend: B) -> io::Result<Self> {
        let size = backend.size()?;
        Ok(Self {
            backend,
            previous: Buffer::empty(Rect::new(0, 0, size.width, size.height)),
            size,
        })
    }

    pub fn draw<F>(&mut self, render: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        let size = self.backend.size()?;
        if size != self.size {
            self.size = size;
            self.previous = Buffer::empty(Rect::new(0, 0, 0, 0));
        }
        let area = Rect::new(0, 0, size.width, size.height);
        let mut buffer = Buffer::empty(area);
        let cursor = {
            let mut frame = Frame {
                area,
                buffer: &mut buffer,
                cursor: None,
            };
            render(&mut frame);
            frame.cursor
        };
        self.backend.draw(&self.previous, &buffer)?;
        self.backend.set_cursor(cursor)?;
        self.backend.flush()?;
        self.previous = buffer;
        Ok(())
    }

    #[must_use]
    pub const fn size(&self) -> Size {
        self.size
    }

    #[must_use]
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    pub const fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

pub type DefaultTerminal = Terminal<AnsiBackend>;

impl Terminal<AnsiBackend> {
    pub fn enter() -> io::Result<Self> {
        Self::new(AnsiBackend::enter()?)
    }

    pub fn restore(&mut self) -> io::Result<()> {
        restore_terminal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::tui::style::Style;

    #[test]
    fn peer_text_cannot_inject_control_sequences_into_the_byte_stream() {
        let area = Rect::new(0, 0, 60, 1);
        let mut current = Buffer::empty(area);
        current.set_stringn(
            0,
            0,
            "hi \x1b]0;pwned\x07 \x1b[2J \u{9b}31m \x07 done",
            60,
            Style::default(),
        );
        let bytes = encode_changes(&Buffer::empty(area), &current);

        // Blank cells are not rewritten, so the text arrives in pieces; the
        // neutralised sequences are shown, not dropped.
        assert!(bytes.contains("hi"), "{bytes:?}");
        assert!(bytes.contains("\u{FFFD}]0;pwned\u{FFFD}"), "{bytes:?}");
        assert!(bytes.contains("done"), "{bytes:?}");
        assert!(!bytes.contains("\x1b]"), "OSC reached the terminal: {bytes:?}");
        assert!(!bytes.contains('\x07'), "BEL reached the terminal: {bytes:?}");
        assert!(!bytes.contains("\x1b[2J"), "CSI 2J reached the terminal: {bytes:?}");
        assert!(!bytes.contains('\u{9b}'), "8-bit CSI reached the terminal: {bytes:?}");
        // What remains is the backend's own cursor moves and style changes:
        // every ESC is followed by `[` and ends in `H` or `m`.
        for sequence in bytes.split('\x1b').skip(1) {
            let body = sequence.strip_prefix('[').expect("only CSI sequences");
            let end = body.find(|c: char| c.is_ascii_alphabetic()).expect("terminated");
            assert!(matches!(&body[end..=end], "H" | "m"), "{bytes:?}");
        }
    }
}
