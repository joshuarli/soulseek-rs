//! Keyboard and mouse events decoded from the terminal byte stream.

use rustix::event::{self, FdSetElement, Timespec};
use std::collections::VecDeque;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const ESCAPE_TIMEOUT: Duration = Duration::from_millis(30);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyModifiers(u8);

impl KeyModifiers {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1);
    pub const CONTROL: Self = Self(2);
    pub const ALT: Self = Self(4);
    pub const SUPER: Self = Self(8);
    pub const HYPER: Self = Self(16);
    pub const META: Self = Self(32);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for KeyModifiers {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for KeyModifiers {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyCode {
    Backspace,
    Enter,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Tab,
    BackTab,
    Delete,
    Insert,
    F(u8),
    Char(char),
    Esc,
    #[default]
    Null,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum KeyEventKind {
    #[default]
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
    pub kind: KeyEventKind,
}

impl KeyEvent {
    #[must_use]
    pub const fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self {
            code,
            modifiers,
            kind: KeyEventKind::Press,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseEventKind {
    Down(MouseButton),
    Up(MouseButton),
    Drag(MouseButton),
    Moved,
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    pub column: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}

impl Default for MouseEventKind {
    fn default() -> Self {
        Self::Down(MouseButton::Left)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Key(KeyEvent),
    Mouse(MouseEvent),
}

static INPUT: Mutex<Input> = Mutex::new(Input {
    decoder: Decoder {
        bytes: Vec::new(),
        escape_since: None,
    },
    events: VecDeque::new(),
});

struct Input {
    decoder: Decoder,
    events: VecDeque<Event>,
}

struct Decoder {
    bytes: Vec<u8>,
    escape_since: Option<Instant>,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8]) {
        if self.bytes.is_empty() {
            self.escape_since = bytes.first().filter(|byte| **byte == 0x1b).map(|_| Instant::now());
        }
        self.bytes.extend_from_slice(bytes);
    }

    fn next(&mut self, now: Instant) -> Option<Event> {
        if self.bytes.is_empty() {
            self.escape_since = None;
            return None;
        }
        if self.bytes[0] != 0x1b {
            self.escape_since = None;
            if let Some(event) = decode_one(&mut self.bytes) {
                return Some(event);
            }
            return None;
        }
        if self.bytes.len() == 1 {
            if self
                .escape_since
                .is_some_and(|started| now.duration_since(started) >= ESCAPE_TIMEOUT)
            {
                self.bytes.remove(0);
                self.escape_since = None;
                return Some(key(KeyCode::Esc, KeyModifiers::NONE));
            }
            return None;
        }
        match self.bytes[1] {
            b'[' => {
                if let Some((event, length)) = decode_csi(&self.bytes) {
                    self.bytes.drain(..length);
                    self.escape_since = None;
                    return Some(event);
                }
                if self
                    .escape_since
                    .is_some_and(|started| now.duration_since(started) >= ESCAPE_TIMEOUT)
                {
                    self.bytes.remove(0);
                    self.escape_since = None;
                    return Some(key(KeyCode::Esc, KeyModifiers::NONE));
                }
                None
            }
            b'O' => {
                if let Some((event, length)) = decode_ss3(&self.bytes) {
                    self.bytes.drain(..length);
                    self.escape_since = None;
                    return Some(event);
                }
                if self
                    .escape_since
                    .is_some_and(|started| now.duration_since(started) >= ESCAPE_TIMEOUT)
                {
                    self.bytes.remove(0);
                    self.escape_since = None;
                    return Some(key(KeyCode::Esc, KeyModifiers::NONE));
                }
                None
            }
            _ => {
                if let Some((character, length)) = decode_utf8(&self.bytes[1..]) {
                    self.bytes.drain(..=length);
                    self.escape_since = None;
                    return Some(key(
                        KeyCode::Char(character),
                        KeyModifiers::ALT,
                    ));
                }
                self.bytes.remove(0);
                self.escape_since = None;
                Some(key(KeyCode::Esc, KeyModifiers::NONE))
            }
        }
    }

    fn wait(&self, now: Instant) -> Option<Duration> {
        self.escape_since
            .map(|started| ESCAPE_TIMEOUT.saturating_sub(now.duration_since(started)))
    }
}

const fn key(code: KeyCode, modifiers: KeyModifiers) -> Event {
    Event::Key(KeyEvent::new(code, modifiers))
}

fn decode_one(bytes: &mut Vec<u8>) -> Option<Event> {
    let first = *bytes.first()?;
    match first {
        0x00 => {
            bytes.remove(0);
            Some(key(KeyCode::Char(' '), KeyModifiers::CONTROL))
        }
        0x08 | 0x7f => {
            bytes.remove(0);
            Some(key(KeyCode::Backspace, KeyModifiers::NONE))
        }
        0x09 => {
            bytes.remove(0);
            Some(key(KeyCode::Tab, KeyModifiers::NONE))
        }
        0x0a | 0x0d => {
            bytes.remove(0);
            Some(key(KeyCode::Enter, KeyModifiers::NONE))
        }
        0x01..=0x1a => {
            bytes.remove(0);
            let letter = char::from(b'a' + first - 1);
            Some(key(KeyCode::Char(letter), KeyModifiers::CONTROL))
        }
        _ => {
            let (character, length) = decode_utf8(bytes)?;
            bytes.drain(..length);
            Some(key(KeyCode::Char(character), KeyModifiers::NONE))
        }
    }
}

fn decode_utf8(bytes: &[u8]) -> Option<(char, usize)> {
    let first = *bytes.first()?;
    let length = match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => {
            return None;
        }
    };
    if bytes.len() < length {
        return None;
    }
    let text = std::str::from_utf8(&bytes[..length]).ok()?;
    text.chars().next().map(|character| (character, length))
}

fn decode_ss3(bytes: &[u8]) -> Option<(Event, usize)> {
    let code = *bytes.get(2)?;
    let (key_code, modifiers) = match code {
        b'A' => (KeyCode::Up, KeyModifiers::NONE),
        b'B' => (KeyCode::Down, KeyModifiers::NONE),
        b'C' => (KeyCode::Right, KeyModifiers::NONE),
        b'D' => (KeyCode::Left, KeyModifiers::NONE),
        b'H' => (KeyCode::Home, KeyModifiers::NONE),
        b'F' => (KeyCode::End, KeyModifiers::NONE),
        b'P' => (KeyCode::F(1), KeyModifiers::NONE),
        b'Q' => (KeyCode::F(2), KeyModifiers::NONE),
        b'R' => (KeyCode::F(3), KeyModifiers::NONE),
        b'S' => (KeyCode::F(4), KeyModifiers::NONE),
        _ => return None,
    };
    Some((key(key_code, modifiers), 3))
}

fn decode_csi(bytes: &[u8]) -> Option<(Event, usize)> {
    let final_index = bytes
        .iter()
        .enumerate()
        .skip(2)
        .find(|(_, byte)| (0x40..=0x7e).contains(*byte))
        .map(|(index, _)| index)?;
    let final_byte = bytes[final_index];
    let params = std::str::from_utf8(&bytes[2..final_index]).ok()?;
    let length = final_index + 1;
    if (final_byte == b'M' || final_byte == b'm')
        && let Some(rest) = params.strip_prefix('<')
    {
        return decode_sgr_mouse(rest, final_byte == b'm')
            .map(|event| (event, length));
    }
    let modifier = params
        .split(';')
        .nth(1)
        .and_then(|value| value.parse::<u8>().ok())
        .map_or(KeyModifiers::NONE, modifier_bits);
    let code = match final_byte {
        b'A' => Some(KeyCode::Up),
        b'B' => Some(KeyCode::Down),
        b'C' => Some(KeyCode::Right),
        b'D' => Some(KeyCode::Left),
        b'H' => Some(KeyCode::Home),
        b'F' => Some(KeyCode::End),
        b'Z' => Some(KeyCode::BackTab),
        b'P' => Some(KeyCode::F(1)),
        b'Q' => Some(KeyCode::F(2)),
        b'R' => Some(KeyCode::F(3)),
        b'S' => Some(KeyCode::F(4)),
        b'~' => {
            let number = params
                .split(';')
                .next()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(0);
            Some(match number {
                1 | 7 => KeyCode::Home,
                2 => KeyCode::Insert,
                3 => KeyCode::Delete,
                4 | 8 => KeyCode::End,
                5 => KeyCode::PageUp,
                6 => KeyCode::PageDown,
                11 => KeyCode::F(1),
                12 => KeyCode::F(2),
                13 => KeyCode::F(3),
                14 => KeyCode::F(4),
                15 => KeyCode::F(5),
                17 => KeyCode::F(6),
                18 => KeyCode::F(7),
                19 => KeyCode::F(8),
                20 => KeyCode::F(9),
                21 => KeyCode::F(10),
                23 => KeyCode::F(11),
                24 => KeyCode::F(12),
                _ => KeyCode::Null,
            })
        }
        _ => None,
    };
    code.map(|code| (key(code, modifier), length))
}

const fn modifier_bits(value: u8) -> KeyModifiers {
    let mut modifiers = KeyModifiers::NONE;
    let value = value.saturating_sub(1);
    if value & 1 != 0 {
        modifiers = KeyModifiers::SHIFT;
    }
    if value & 2 != 0 {
        modifiers = modifiers_bitor(modifiers, KeyModifiers::ALT);
    }
    if value & 4 != 0 {
        modifiers = modifiers_bitor(modifiers, KeyModifiers::CONTROL);
    }
    if value & 8 != 0 {
        modifiers = modifiers_bitor(modifiers, KeyModifiers::SUPER);
    }
    modifiers
}

const fn modifiers_bitor(left: KeyModifiers, right: KeyModifiers) -> KeyModifiers {
    KeyModifiers(left.0 | right.0)
}

fn decode_sgr_mouse(params: &str, release: bool) -> Option<Event> {
    let mut fields = params.split(';');
    let button = fields.next()?.parse::<u8>().ok()?;
    let column = fields.next()?.parse::<u16>().ok()?.saturating_sub(1);
    let row = fields.next()?.parse::<u16>().ok()?.saturating_sub(1);
    let mut modifiers = KeyModifiers::NONE;
    if button & 4 != 0 {
        modifiers |= KeyModifiers::SHIFT;
    }
    if button & 8 != 0 {
        modifiers |= KeyModifiers::ALT;
    }
    if button & 16 != 0 {
        modifiers |= KeyModifiers::CONTROL;
    }
    let wheel = button & 64 != 0;
    let motion = button & 32 != 0;
    let kind = if wheel {
        match button & 3 {
            0 => MouseEventKind::ScrollUp,
            1 => MouseEventKind::ScrollDown,
            2 => MouseEventKind::ScrollLeft,
            _ => MouseEventKind::ScrollRight,
        }
    } else if release {
        MouseEventKind::Up(button_from_bits(button))
    } else if motion {
        if button & 3 == 3 {
            MouseEventKind::Moved
        } else {
            MouseEventKind::Drag(button_from_bits(button))
        }
    } else {
        MouseEventKind::Down(button_from_bits(button))
    };
    Some(Event::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers,
    }))
}

const fn button_from_bits(bits: u8) -> MouseButton {
    match bits & 3 {
        0 => MouseButton::Left,
        1 => MouseButton::Middle,
        _ => MouseButton::Right,
    }
}

pub fn poll(timeout: Duration) -> io::Result<bool> {
    let started = Instant::now();
    let mut input = INPUT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        let now = Instant::now();
        if let Some(event) = input.decoder.next(now) {
            input.events.push_back(event);
            return Ok(true);
        }
        let elapsed = now.duration_since(started);
        let mut wait = timeout.saturating_sub(elapsed);
        if let Some(escape) = input.decoder.wait(now) {
            wait = wait.min(escape);
        }
        if wait.is_zero() {
            if elapsed >= timeout {
                return Ok(false);
            }
            continue;
        }
        if input_ready(wait)? {
            read_bytes(&mut input)?;
        }
    }
}

pub fn read() -> io::Result<Event> {
    let mut input = INPUT.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    loop {
        if let Some(event) = input.events.pop_front() {
            return Ok(event);
        }
        let now = Instant::now();
        if let Some(event) = input.decoder.next(now) {
            return Ok(event);
        }
        let wait = input.decoder.wait(now);
        if wait.is_some_and(|wait| wait.is_zero())
            && let Some(event) = input.decoder.next(Instant::now())
        {
            return Ok(event);
        }
        if let Some(wait) = wait
            && input_ready(wait)?
        {
            read_bytes(&mut input)?;
            continue;
        }
        read_bytes(&mut input)?;
    }
}

fn read_bytes(input: &mut Input) -> io::Result<()> {
    let stdin = io::stdin();
    let mut bytes = [0_u8; 256];
    match rustix::io::read(&stdin, &mut bytes) {
        Ok(0) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "terminal input closed",
        )),
        Ok(length) => {
            input.decoder.feed(&bytes[..length]);
            Ok(())
        }
        Err(rustix::io::Errno::INTR) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn input_ready(timeout: Duration) -> io::Result<bool> {
    let stdin = io::stdin();
    let descriptor: RawFd = stdin.as_raw_fd();
    let mut descriptors =
        vec![FdSetElement::default(); event::fd_set_num_elements(1, descriptor + 1)];
    event::fd_set_insert(&mut descriptors, descriptor);
    let timeout = Timespec::try_from(timeout).map_err(|_| {
        io::Error::new(io::ErrorKind::InvalidInput, "input timeout too large")
    })?;
    // SAFETY: the only descriptor in the set is borrowed from the live stdin
    // handle, and the timeout is a local value that outlives the call.
    let ready = unsafe {
        event::select(
            descriptor + 1,
            Some(&mut descriptors),
            None,
            None,
            Some(&timeout),
        )
    }?;
    Ok(ready != 0)
}
