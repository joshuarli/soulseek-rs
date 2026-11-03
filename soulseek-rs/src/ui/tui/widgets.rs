//! The widgets the window draws with.

use super::buffer::Buffer;
use super::layout::{Alignment, Constraint, Flex, Layout, Rect};
use super::style::Style;
use super::text::{Line, Span, Text};
use std::collections::VecDeque;
use unicode_width::UnicodeWidthChar;

pub trait Widget {
    fn render(self, area: Rect, buffer: &mut Buffer);
}

pub trait StatefulWidget {
    type State;

    fn render(self, area: Rect, buffer: &mut Buffer, state: &mut Self::State);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Borders(u8);

impl Borders {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1);
    pub const RIGHT: Self = Self(2);
    pub const BOTTOM: Self = Self(4);
    pub const LEFT: Self = Self(8);
    pub const ALL: Self = Self(15);

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl std::ops::BitOr for Borders {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BorderType {
    #[default]
    Plain,
    Rounded,
}

impl BorderType {
    const fn corners(self) -> [char; 4] {
        match self {
            Self::Plain => ['┌', '┐', '└', '┘'],
            Self::Rounded => ['╭', '╮', '╰', '╯'],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Padding {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

impl Padding {
    #[must_use]
    pub const fn horizontal(value: u16) -> Self {
        Self {
            left: value,
            right: value,
            top: 0,
            bottom: 0,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Block<'a> {
    pub borders: Borders,
    pub border_style: Style,
    pub border_type: BorderType,
    pub padding: Padding,
    pub style: Style,
    pub titles: Vec<Line<'a>>,
}

impl<'a> Block<'a> {
    #[must_use]
    pub const fn borders(mut self, borders: Borders) -> Self {
        self.borders = borders;
        self
    }

    #[must_use]
    pub const fn border_style(mut self, style: Style) -> Self {
        self.border_style = style;
        self
    }

    #[must_use]
    pub const fn border_type(mut self, border_type: BorderType) -> Self {
        self.border_type = border_type;
        self
    }

    #[must_use]
    pub const fn padding(mut self, padding: Padding) -> Self {
        self.padding = padding;
        self
    }

    #[must_use]
    pub fn title(mut self, title: impl Into<Line<'a>>) -> Self {
        self.titles.push(title.into());
        self
    }

    #[must_use]
    pub fn inner(&self, area: Rect) -> Rect {
        let mut inner = area;
        if self.borders.intersects(Borders::LEFT) {
            inner.x = inner.x.saturating_add(1).min(inner.right());
            inner.width = inner.width.saturating_sub(1);
        }
        if self.borders.intersects(Borders::TOP) || !self.titles.is_empty() {
            inner.y = inner.y.saturating_add(1).min(inner.bottom());
            inner.height = inner.height.saturating_sub(1);
        }
        if self.borders.intersects(Borders::RIGHT) {
            inner.width = inner.width.saturating_sub(1);
        }
        if self.borders.intersects(Borders::BOTTOM) {
            inner.height = inner.height.saturating_sub(1);
        }
        inner.x = inner.x.saturating_add(self.padding.left);
        inner.y = inner.y.saturating_add(self.padding.top);
        inner.width = inner.width.saturating_sub(
            self.padding.left.saturating_add(self.padding.right),
        );
        inner.height = inner.height.saturating_sub(
            self.padding.top.saturating_add(self.padding.bottom),
        );
        inner
    }
}

impl Widget for Block<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        let area = area.intersection(buffer.area);
        if area.is_empty() {
            return;
        }
        buffer.set_style(area, self.style);
        self.render_borders(area, buffer);
        self.render_titles(area, buffer);
    }
}

impl Block<'_> {
    fn render_borders(&self, area: Rect, buffer: &mut Buffer) {
        if self.borders == Borders::NONE {
            return;
        }
        let horizontal = '─';
        let vertical = '│';
        if self.borders.intersects(Borders::TOP) {
            for x in area.x..area.right() {
                buffer.set_stringn(x, area.y, &horizontal.to_string(), 1, self.border_style);
            }
        }
        if self.borders.intersects(Borders::BOTTOM) {
            for x in area.x..area.right() {
                buffer.set_stringn(
                    x,
                    area.bottom().saturating_sub(1),
                    &horizontal.to_string(),
                    1,
                    self.border_style,
                );
            }
        }
        if self.borders.intersects(Borders::LEFT) {
            for y in area.y..area.bottom() {
                buffer.set_stringn(area.x, y, &vertical.to_string(), 1, self.border_style);
            }
        }
        if self.borders.intersects(Borders::RIGHT) {
            for y in area.y..area.bottom() {
                buffer.set_stringn(
                    area.right().saturating_sub(1),
                    y,
                    &vertical.to_string(),
                    1,
                    self.border_style,
                );
            }
        }
        let corners = self.border_type.corners();
        let right = area.right().saturating_sub(1);
        let bottom = area.bottom().saturating_sub(1);
        if self.borders.contains(Borders::TOP | Borders::LEFT) {
            buffer.set_stringn(area.x, area.y, &corners[0].to_string(), 1, self.border_style);
        }
        if self.borders.contains(Borders::TOP | Borders::RIGHT) {
            buffer.set_stringn(right, area.y, &corners[1].to_string(), 1, self.border_style);
        }
        if self.borders.contains(Borders::BOTTOM | Borders::LEFT) {
            buffer.set_stringn(area.x, bottom, &corners[2].to_string(), 1, self.border_style);
        }
        if self.borders.contains(Borders::BOTTOM | Borders::RIGHT) {
            buffer.set_stringn(right, bottom, &corners[3].to_string(), 1, self.border_style);
        }
    }

    fn render_titles(&self, area: Rect, buffer: &mut Buffer) {
        if self.titles.is_empty() || area.is_empty() {
            return;
        }
        let left = u16::from(self.borders.contains(Borders::LEFT));
        let right = u16::from(self.borders.contains(Borders::RIGHT));
        let mut cursor = area.x.saturating_add(left);
        let mut width = area.width.saturating_sub(left).saturating_sub(right);
        for title in &self.titles {
            if width == 0 {
                break;
            }
            let title_width = u16::try_from(title.width()).unwrap_or(u16::MAX);
            buffer.set_line(cursor, area.y, title, width);
            let advance = title_width.saturating_add(1);
            cursor = cursor.saturating_add(advance);
            width = width.saturating_sub(advance);
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Wrap {
    pub trim: bool,
}

#[derive(Clone, Debug)]
pub struct Paragraph<'a> {
    pub text: Text<'a>,
    pub block: Option<Block<'a>>,
    pub style: Style,
    pub wrap: Option<Wrap>,
    pub scroll: (u16, u16),
    pub alignment: Option<Alignment>,
}

impl<'a> Paragraph<'a> {
    #[must_use]
    pub fn new(text: impl Into<Text<'a>>) -> Self {
        Self {
            text: text.into(),
            block: None,
            style: Style::default(),
            wrap: None,
            scroll: (0, 0),
            alignment: None,
        }
    }

    #[must_use]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    #[must_use]
    pub const fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    #[must_use]
    pub const fn wrap(mut self, wrap: Wrap) -> Self {
        self.wrap = Some(wrap);
        self
    }

    #[must_use]
    pub const fn scroll(mut self, offset: (u16, u16)) -> Self {
        self.scroll = offset;
        self
    }

    #[must_use]
    pub const fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = Some(alignment);
        self
    }
}

impl Widget for Paragraph<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        buffer.set_style(area.intersection(buffer.area), self.style);
        let inner = if let Some(block) = self.block.clone() {
            let inner = block.inner(area);
            block.render(area, buffer);
            inner
        } else {
            area
        };
        buffer.set_style(inner.intersection(buffer.area), self.style);
        render_paragraph(&self, inner, buffer);
    }
}

fn render_paragraph(paragraph: &Paragraph<'_>, area: Rect, buffer: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let mut rows: Vec<Line<'_>> = Vec::new();
    if let Some(wrap) = paragraph.wrap {
        for line in &paragraph.text.lines {
            rows.extend(wrap_line(line, width, wrap.trim));
        }
    } else {
        rows.extend(paragraph.text.lines.iter().cloned());
    }
    let skip_rows = usize::from(paragraph.scroll.0);
    let skip_columns = usize::from(paragraph.scroll.1);
    for (row_index, line) in rows.iter().enumerate().skip(skip_rows) {
        let y = area
            .y
            .saturating_add(u16::try_from(row_index - skip_rows).unwrap_or(u16::MAX));
        if y >= area.bottom() {
            break;
        }
        render_line(
            line,
            area,
            y,
            skip_columns,
            paragraph.alignment.or(line.alignment),
            paragraph.style,
            buffer,
        );
    }
}

fn render_line(
    line: &Line<'_>,
    area: Rect,
    y: u16,
    skip_columns: usize,
    alignment: Option<Alignment>,
    base_style: Style,
    buffer: &mut Buffer,
) {
    let line_width = line.width();
    let visible_width = line_width.saturating_sub(skip_columns).min(usize::from(area.width));
    let x = match alignment.unwrap_or(Alignment::Left) {
        Alignment::Left => area.x,
        Alignment::Center => area
            .x
            .saturating_add(u16::try_from((usize::from(area.width) - visible_width) / 2).unwrap_or(0)),
        Alignment::Right => area
            .right()
            .saturating_sub(u16::try_from(visible_width).unwrap_or(u16::MAX)),
    };
    let mut cursor = x;
    let mut skipped = 0_usize;
    for span in &line.spans {
        let span_width = span.width();
        if skipped + span_width <= skip_columns {
            skipped += span_width;
            continue;
        }
        let mut text = span.content.as_ref();
        if skipped < skip_columns {
            let mut consumed = 0_usize;
            let mut byte = 0_usize;
            for character in text.chars() {
                let cells = UnicodeWidthChar::width(character).unwrap_or(0);
                if skipped + consumed + cells > skip_columns {
                    break;
                }
                consumed += cells;
                byte += character.len_utf8();
            }
            text = &text[byte..];
            skipped = skip_columns;
        }
        let remaining = area.right().saturating_sub(cursor);
        if remaining == 0 {
            break;
        }
        let style = base_style.patch(line.style).patch(span.style);
        cursor = buffer.set_stringn(cursor, y, text, remaining, style);
    }
}

fn wrap_line<'a>(line: &Line<'a>, width: usize, trim: bool) -> Vec<Line<'a>> {
    if width == 0 {
        return Vec::new();
    }
    let mut rows: Vec<Line<'a>> = Vec::new();
    let mut pending_line: Vec<(char, Style)> = Vec::new();
    let mut line_width = 0_usize;
    let mut pending_word: Vec<(char, Style)> = Vec::new();
    let mut pending_whitespace: VecDeque<(char, Style)> = VecDeque::new();
    let mut word_width = 0_usize;
    let mut whitespace_width = 0_usize;
    let mut non_whitespace_previous = false;

    for span in &line.spans {
        for character in span.content.chars() {
            let is_whitespace = character.is_whitespace();
            let cells = UnicodeWidthChar::width(character).unwrap_or(0);
            if cells > width {
                continue;
            }

            let word_found = non_whitespace_previous && is_whitespace;
            let trimmed_overflow =
                pending_line.is_empty() && trim && word_width + cells > width;
            let whitespace_overflow = pending_line.is_empty()
                && trim
                && whitespace_width + cells > width;
            let untrimmed_overflow = pending_line.is_empty()
                && !trim
                && word_width + whitespace_width + cells > width;

            if word_found
                || trimmed_overflow
                || whitespace_overflow
                || untrimmed_overflow
            {
                if !pending_line.is_empty() || !trim {
                    pending_line
                        .extend(std::mem::take(&mut pending_whitespace));
                    line_width += whitespace_width;
                }
                pending_line.append(&mut pending_word);
                line_width += word_width;
                pending_whitespace.clear();
                whitespace_width = 0;
                word_width = 0;
            }

            let line_full = line_width >= width;
            let pending_word_overflow = cells > 0
                && line_width + whitespace_width + word_width >= width;

            if line_full || pending_word_overflow {
                let mut remaining_width = width.saturating_sub(line_width);
                rows.push(glyphs_to_line(
                    std::mem::take(&mut pending_line),
                    line.style,
                    line.alignment,
                ));
                line_width = 0;

                while let Some(&(whitespace, _)) = pending_whitespace.front() {
                    let cells =
                        UnicodeWidthChar::width(whitespace).unwrap_or(0);
                    if cells > remaining_width {
                        break;
                    }
                    whitespace_width =
                        whitespace_width.saturating_sub(cells);
                    remaining_width -= cells;
                    pending_whitespace.pop_front();
                }

                if is_whitespace && pending_whitespace.is_empty() {
                    continue;
                }
            }

            if is_whitespace {
                whitespace_width += cells;
                pending_whitespace.push_back((character, span.style));
            } else {
                word_width += cells;
                pending_word.push((character, span.style));
            }
            non_whitespace_previous = !is_whitespace;
        }
    }

    if pending_line.is_empty()
        && pending_word.is_empty()
        && !pending_whitespace.is_empty()
        && trim
    {
        rows.push(Line::default());
    }
    if !pending_line.is_empty() || !trim {
        pending_line.extend(std::mem::take(&mut pending_whitespace));
    }
    pending_line.append(&mut pending_word);

    if pending_line.is_empty() {
        if rows.is_empty() {
            rows.push(Line::default());
        }
    } else {
        rows.push(glyphs_to_line(
            pending_line,
            line.style,
            line.alignment,
        ));
    }
    rows
}

fn glyphs_to_line<'a>(
    glyphs: Vec<(char, Style)>,
    style: Style,
    alignment: Option<Alignment>,
) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    for (character, glyph_style) in glyphs {
        match spans.last_mut() {
            Some(span) if span.style == glyph_style => {
                span.content.to_mut().push(character);
            }
            _ => spans.push(Span::styled(character.to_string(), glyph_style)),
        }
    }
    Line {
        spans,
        style,
        alignment,
    }
}

pub struct Clear;

impl Widget for Clear {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        buffer.reset(area.intersection(buffer.area));
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HighlightSpacing {
    Always,
    #[default]
    WhenSelected,
}

impl HighlightSpacing {
    const fn should_add(self, selected: bool) -> bool {
        match self {
            Self::Always => true,
            Self::WhenSelected => selected,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Cell<'a> {
    pub content: Text<'a>,
    pub style: Style,
}

impl<'a> Cell<'a> {
    #[must_use]
    pub fn from(content: impl Into<Text<'a>>) -> Self {
        Self {
            content: content.into(),
            style: Style::default(),
        }
    }

    #[must_use]
    pub const fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

#[derive(Clone, Debug)]
pub struct Row<'a> {
    pub cells: Vec<Cell<'a>>,
    pub style: Style,
    pub height: u16,
}

impl<'a> Row<'a> {
    #[must_use]
    pub fn new(cells: impl Into<Vec<Cell<'a>>>) -> Self {
        Self {
            cells: cells.into(),
            style: Style::default(),
            height: 1,
        }
    }

    #[must_use]
    pub const fn height(mut self, height: u16) -> Self {
        self.height = height;
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableState {
    selected: Option<usize>,
    offset: usize,
}

impl TableState {
    #[must_use]
    pub const fn with_selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    pub const fn select(&mut self, selected: Option<usize>) {
        self.selected = selected;
    }

    #[must_use]
    pub const fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub const fn selected_mut(&mut self) -> &mut Option<usize> {
        &mut self.selected
    }

    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    pub const fn offset_mut(&mut self) -> &mut usize {
        &mut self.offset
    }
}

#[derive(Clone, Debug)]
pub struct Table<'a> {
    pub rows: Vec<Row<'a>>,
    pub widths: Vec<Constraint>,
    pub header: Option<Row<'a>>,
    pub block: Option<Block<'a>>,
    pub row_highlight_style: Style,
    pub highlight_symbol: Line<'a>,
    pub highlight_spacing: HighlightSpacing,
    pub column_spacing: u16,
}

impl<'a> Table<'a> {
    #[must_use]
    pub fn new(
        rows: impl Into<Vec<Row<'a>>>,
        widths: impl Into<Vec<Constraint>>,
    ) -> Self {
        Self {
            rows: rows.into(),
            widths: widths.into(),
            header: None,
            block: None,
            row_highlight_style: Style::default(),
            highlight_symbol: Line::default(),
            highlight_spacing: HighlightSpacing::default(),
            column_spacing: 1,
        }
    }

    #[must_use]
    pub fn header(mut self, header: Row<'a>) -> Self {
        self.header = Some(header);
        self
    }

    #[must_use]
    pub fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    #[must_use]
    pub const fn row_highlight_style(mut self, style: Style) -> Self {
        self.row_highlight_style = style;
        self
    }

    #[must_use]
    pub fn highlight_symbol(mut self, symbol: impl Into<Line<'a>>) -> Self {
        self.highlight_symbol = symbol.into();
        self
    }

    #[must_use]
    pub const fn highlight_spacing(mut self, spacing: HighlightSpacing) -> Self {
        self.highlight_spacing = spacing;
        self
    }
}

impl StatefulWidget for Table<'_> {
    type State = TableState;

    fn render(self, area: Rect, buffer: &mut Buffer, state: &mut Self::State) {
        if self.rows.is_empty() {
            state.select(None);
        }
        let inner = if let Some(block) = self.block.clone() {
            let inner = block.inner(area);
            block.render(area, buffer);
            inner
        } else {
            area
        };
        if inner.is_empty() {
            return;
        }
        let header_height = self.header.as_ref().map_or(0, |header| header.height);
        let header_height = header_height.min(inner.height);
        let body_area = Rect {
            y: inner.y.saturating_add(header_height),
            height: inner.height.saturating_sub(header_height),
            ..inner
        };
        let has_selection = state.selected.is_some();
        let selection_width = if self.highlight_spacing.should_add(has_selection) {
            u16::try_from(self.highlight_symbol.width()).unwrap_or(u16::MAX)
        } else {
            0
        };
        let columns_area = layout_columns(inner.width, selection_width);
        let widths = column_widths(
            columns_area,
            &self.widths,
            self.rows_cells(),
            self.column_spacing,
        );

        if let Some(header) = self.header.as_ref() {
            buffer.set_style(
                Rect {
                    height: header_height,
                    ..inner
                },
                header.style,
            );
            for (column, cell) in widths.iter().zip(&header.cells) {
                render_cell(
                    cell,
                    Rect {
                        x: inner
                            .x
                            .saturating_add(columns_area.x)
                            .saturating_add(column.0),
                        y: inner.y,
                        width: column.1,
                        height: header_height,
                    },
                    buffer,
                );
            }
        }

        let (start, end) = visible_rows(&self.rows, state, body_area.height);
        state.offset = start;
        let mut y = body_area.y;
        for index in start..end {
            let entry = &self.rows[index];
            if y >= body_area.bottom() {
                break;
            }
            let height = entry.height.min(body_area.bottom().saturating_sub(y));
            let row_area = Rect {
                y,
                height,
                ..body_area
            };
            buffer.set_style(row_area, entry.style);
            let selected = state.selected == Some(index);
            if selected && selection_width > 0 {
                buffer.set_style(
                    Rect {
                        width: selection_width,
                        ..row_area
                    },
                    entry.style,
                );
                buffer.set_line(
                    row_area.x,
                    row_area.y,
                    &self.highlight_symbol,
                    selection_width,
                );
            }
            for (column, cell) in widths.iter().zip(&entry.cells) {
                render_cell(
                    cell,
                    Rect {
                        x: inner
                            .x
                            .saturating_add(columns_area.x)
                            .saturating_add(column.0),
                        y,
                        width: column.1,
                        height,
                    },
                    buffer,
                );
            }
            if selected {
                buffer.set_style(row_area, self.row_highlight_style);
            }
            y = y.saturating_add(entry.height);
        }
    }
}

impl Table<'_> {
    fn rows_cells(&self) -> usize {
        self.rows
            .iter()
            .map(|row| row.cells.len())
            .chain(self.header.iter().map(|row| row.cells.len()))
            .max()
            .unwrap_or(0)
    }
}

fn layout_columns(width: u16, selection_width: u16) -> Rect {
    let [_, columns] =
        Layout::horizontal([Constraint::Length(selection_width), Constraint::Fill(0)])
            .areas(Rect::new(0, 0, width, 1));
    Rect::new(selection_width, 0, columns.width, 1)
}

fn column_widths(
    columns_area: Rect,
    widths: &[Constraint],
    column_count: usize,
    spacing: u16,
) -> Vec<(u16, u16)> {
    let constraints = if widths.is_empty() {
        vec![
            Constraint::Length(columns_area.width / u16::try_from(column_count.max(1)).unwrap_or(1));
            column_count
        ]
    } else {
        widths.to_vec()
    };
    Layout::horizontal(constraints)
        .flex(Flex::Start)
        .spacing(spacing)
        .split(Rect::new(0, 0, columns_area.width, 1))
        .into_iter()
        .map(|area| (area.x, area.width))
        .collect()
}

fn visible_rows(
    rows: &[Row<'_>],
    state: &TableState,
    height: u16,
) -> (usize, usize) {
    let last_row = rows.len().saturating_sub(1);
    let mut start = state.offset.min(last_row);
    if let Some(selected) = state.selected {
        start = start.min(selected.min(last_row));
    }
    let mut end = start;
    let mut used = 0_u16;
    for row in rows.iter().skip(start) {
        if used + row.height > height {
            break;
        }
        used = used.saturating_add(row.height);
        end += 1;
    }
    if let Some(selected) = state.selected {
        let selected = selected.min(last_row);
        while selected >= end && end < rows.len() {
            used = used.saturating_add(rows[end].height);
            end += 1;
            while used > height && start < end {
                used = used.saturating_sub(rows[start].height);
                start += 1;
            }
        }
    }
    if used < height && end < rows.len() {
        end += 1;
    }
    (start, end)
}

fn render_cell(cell: &Cell<'_>, area: Rect, buffer: &mut Buffer) {
    if area.is_empty() {
        return;
    }
    buffer.set_style(area, cell.style);
    for line in &cell.content.lines {
        buffer.set_line(area.x, area.y, line, area.width);
    }
}
