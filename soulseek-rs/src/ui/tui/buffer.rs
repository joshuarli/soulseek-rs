//! A cell buffer and the primitives every widget draws through.

use super::layout::{Position, Rect};
use super::style::Style;
use super::text::Line;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Cell {
    pub symbol: String,
    pub style: Style,
    pub width: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            symbol: " ".to_string(),
            style: Style::default(),
            width: 1,
        }
    }
}

impl Cell {
    fn blank(style: Style) -> Self {
        Self {
            symbol: " ".to_string(),
            style,
            width: 1,
        }
    }

    const fn continuation(style: Style) -> Self {
        Self {
            symbol: String::new(),
            style,
            width: 0,
        }
    }

    #[must_use]
    pub fn width(&self) -> u16 {
        u16::from(self.width)
    }
}

pub struct Buffer {
    pub area: Rect,
    pub content: Vec<Cell>,
}

impl Buffer {
    #[must_use]
    pub fn empty(area: Rect) -> Self {
        Self {
            area,
            content: vec![Cell::default(); usize::from(area.width) * usize::from(area.height)],
        }
    }

    fn index_of(&self, x: u16, y: u16) -> usize {
        usize::from(y) * usize::from(self.area.width) + usize::from(x)
    }

    #[must_use]
    pub fn cell(&self, position: Position) -> Option<&Cell> {
        if !self.area.contains(position) {
            return None;
        }
        self.content
            .get(self.index_of(position.x, position.y))
    }

    pub fn cell_mut(&mut self, position: Position) -> Option<&mut Cell> {
        if !self.area.contains(position) {
            return None;
        }
        let index = self.index_of(position.x, position.y);
        self.content.get_mut(index)
    }

    pub fn reset(&mut self, area: Rect) {
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if let Some(cell) = self.cell_mut(Position::new(x, y)) {
                    *cell = Cell::default();
                }
            }
        }
    }

    pub fn set_style(&mut self, area: Rect, style: Style) {
        let area = self.area.intersection(area);
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                if let Some(cell) = self.cell_mut(Position::new(x, y)) {
                    cell.style = cell.style.patch(style);
                }
            }
        }
    }

    pub fn set_stringn(
        &mut self,
        x: u16,
        y: u16,
        text: &str,
        max_width: u16,
        style: Style,
    ) -> u16 {
        if y >= self.area.bottom() || x >= self.area.right() {
            return x;
        }
        let end = x.saturating_add(max_width).min(self.area.right());
        let mut x = x;
        let mut glyph = String::new();
        for character in text.chars() {
            if glyph.is_empty() {
                glyph.push(character);
                continue;
            }
            let width = UnicodeWidthStr::width(glyph.as_str());
            glyph.push(character);
            if UnicodeWidthChar::width(character) == Some(0)
                || UnicodeWidthStr::width(glyph.as_str()) <= width
            {
                continue;
            }
            glyph.pop();
            if !self.glyph(&mut x, end, y, &glyph, style) {
                return x;
            }
            glyph.clear();
            glyph.push(character);
        }
        if !glyph.is_empty() {
            self.glyph(&mut x, end, y, &glyph, style);
        }
        x
    }

    fn glyph(
        &mut self,
        x: &mut u16,
        end: u16,
        y: u16,
        text: &str,
        style: Style,
    ) -> bool {
        let width = UnicodeWidthStr::width(text);
        if width == 0 {
            return true;
        }
        if usize::from(*x) + width > usize::from(end) {
            return false;
        }
        for offset in 0..width {
            self.erase_wide_glyph(x.saturating_add(offset as u16), y);
        }
        let existing = self
            .cell(Position::new(*x, y))
            .map_or_else(Style::default, |cell| cell.style);
        let style = existing.patch(style);
        if let Some(cell) = self.cell_mut(Position::new(*x, y)) {
            *cell = Cell {
                symbol: text.to_string(),
                style,
                width: u8::try_from(width).unwrap_or(u8::MAX),
            };
        }
        for offset in 1..width {
            if let Some(cell) = self.cell_mut(Position::new(*x + offset as u16, y)) {
                *cell = Cell::continuation(style);
            }
        }
        *x = x.saturating_add(u16::try_from(width).unwrap_or(u16::MAX));
        true
    }

    fn erase_wide_glyph(&mut self, x: u16, y: u16) {
        let mut lead = x;
        while lead > self.area.x
            && self
                .cell(Position::new(lead, y))
                .is_some_and(|cell| cell.width == 0)
        {
            lead -= 1;
        }
        let Some(cell) = self.cell(Position::new(lead, y)).cloned() else {
            return;
        };
        if u16::from(cell.width) <= 1 {
            return;
        }
        for column in lead..(lead + u16::from(cell.width)).min(self.area.right()) {
            if let Some(cell) = self.cell_mut(Position::new(column, y)) {
                *cell = Cell::blank(Style::default());
            }
        }
    }

    pub fn set_line(&mut self, x: u16, y: u16, line: &Line<'_>, max_width: u16) {
        let mut cursor = x;
        for span in &line.spans {
            let remaining = max_width.saturating_sub(cursor.saturating_sub(x));
            if remaining == 0 {
                break;
            }
            let style = line.style.patch(span.style);
            cursor = self.set_stringn(cursor, y, &span.content, remaining, style);
        }
    }

    #[must_use]
    pub fn to_text(&self) -> String {
        let mut output = String::with_capacity(
            usize::from(self.area.width) * usize::from(self.area.height),
        );
        for y in self.area.y..self.area.bottom() {
            for x in self.area.x..self.area.right() {
                let Some(cell) = self.cell(Position::new(x, y)) else {
                    continue;
                };
                if cell.width == 0 {
                    continue;
                }
                output.push_str(&cell.symbol);
            }
            output.push('\n');
        }
        output
    }
}
