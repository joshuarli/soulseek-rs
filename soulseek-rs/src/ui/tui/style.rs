//! Styles and colours for the in-repo terminal renderer.

use std::ops::{BitAnd, BitOr, BitOrAssign, Not};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum Color {
    #[default]
    Reset,
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    Gray,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    White,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Modifier(u16);

impl Modifier {
    pub const BOLD: Self = Self(1 << 0);
    pub const DIM: Self = Self(1 << 1);
    pub const ITALIC: Self = Self(1 << 2);
    pub const UNDERLINED: Self = Self(1 << 3);
    pub const REVERSED: Self = Self(1 << 4);
    pub const CROSSED_OUT: Self = Self(1 << 5);

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl BitOr for Modifier {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Modifier {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Modifier {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl Not for Modifier {
    type Output = Self;

    fn not(self) -> Self::Output {
        Self(!self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct Style {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub add_modifier: Modifier,
    pub sub_modifier: Modifier,
}

impl Style {
    #[must_use]
    pub const fn new(fg: Color, bg: Color) -> Self {
        Self {
            fg: Some(fg),
            bg: Some(bg),
            add_modifier: Modifier::empty(),
            sub_modifier: Modifier::empty(),
        }
    }

    #[must_use]
    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    #[must_use]
    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    #[must_use]
    pub const fn add_modifier(mut self, modifier: Modifier) -> Self {
        self.add_modifier = Modifier(self.add_modifier.0 | modifier.0);
        self
    }

    #[must_use]
    pub const fn remove_modifier(mut self, modifier: Modifier) -> Self {
        self.sub_modifier = Modifier(self.sub_modifier.0 | modifier.0);
        self
    }

    #[must_use]
    pub fn patch(mut self, other: Self) -> Self {
        if other.fg.is_some() {
            self.fg = other.fg;
        }
        if other.bg.is_some() {
            self.bg = other.bg;
        }
        self.add_modifier |= other.add_modifier;
        self.sub_modifier |= other.sub_modifier;
        self
    }

    pub(crate) fn sgr(self) -> String {
        let mut codes = String::from("\x1b[0");
        let modifier = self.add_modifier & !self.sub_modifier;
        if modifier.contains(Modifier::BOLD) {
            codes.push_str(";1");
        }
        if modifier.contains(Modifier::DIM) {
            codes.push_str(";2");
        }
        if modifier.contains(Modifier::ITALIC) {
            codes.push_str(";3");
        }
        if modifier.contains(Modifier::UNDERLINED) {
            codes.push_str(";4");
        }
        if modifier.contains(Modifier::REVERSED) {
            codes.push_str(";7");
        }
        if modifier.contains(Modifier::CROSSED_OUT) {
            codes.push_str(";9");
        }
        if let Some(color) = self.fg {
            push_color(&mut codes, color, false);
        }
        if let Some(color) = self.bg {
            push_color(&mut codes, color, true);
        }
        codes.push('m');
        codes
    }
}

fn push_color(codes: &mut String, color: Color, background: bool) {
    let base = if background { 40 } else { 30 };
    let bright = if background { 100 } else { 90 };
    let code = match color {
        Color::Reset => return,
        Color::Black => base,
        Color::Red => base + 1,
        Color::Green => base + 2,
        Color::Yellow => base + 3,
        Color::Blue => base + 4,
        Color::Magenta => base + 5,
        Color::Cyan => base + 6,
        Color::Gray => base + 7,
        Color::DarkGray => bright,
        Color::LightRed => bright + 1,
        Color::LightGreen => bright + 2,
        Color::LightYellow => bright + 3,
        Color::LightBlue => bright + 4,
        Color::LightMagenta => bright + 5,
        Color::LightCyan => bright + 6,
        Color::White => bright + 7,
        Color::Rgb(red, green, blue) => {
            let _ = std::fmt::Write::write_fmt(
                codes,
                format_args!(
                    ";{};2;{red};{green};{blue}",
                    if background { 48 } else { 38 }
                ),
            );
            return;
        }
        Color::Indexed(index) => {
            let _ = std::fmt::Write::write_fmt(
                codes,
                format_args!(";{};5;{index}", if background { 48 } else { 38 }),
            );
            return;
        }
    };
    let _ = std::fmt::Write::write_fmt(codes, format_args!(";{code}"));
}
