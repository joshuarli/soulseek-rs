//! Styled text used by the renderer.

use super::style::Style;
use super::layout::Alignment;
use std::borrow::Cow;
use std::fmt;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Span<'a> {
    pub content: Cow<'a, str>,
    pub style: Style,
}

impl<'a> Span<'a> {
    #[must_use]
    pub fn raw(content: impl Into<Cow<'a, str>>) -> Self {
        Self {
            content: content.into(),
            style: Style::default(),
        }
    }

    #[must_use]
    pub fn styled(content: impl Into<Cow<'a, str>>, style: Style) -> Self {
        Self {
            content: content.into(),
            style,
        }
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.content.width()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Line<'a> {
    pub spans: Vec<Span<'a>>,
    pub style: Style,
    pub alignment: Option<Alignment>,
}

impl<'a> Line<'a> {
    #[must_use]
    pub fn raw(content: impl Into<Cow<'a, str>>) -> Self {
        Self::from(Span::raw(content))
    }

    #[must_use]
    pub fn styled(content: impl Into<Cow<'a, str>>, style: Style) -> Self {
        Self::from(Span::styled(content, style))
    }

    #[must_use]
    pub fn width(&self) -> usize {
        self.spans.iter().map(Span::width).sum()
    }

    #[must_use]
    pub const fn alignment(mut self, alignment: Alignment) -> Self {
        self.alignment = Some(alignment);
        self
    }
}

impl<'a> From<&'a str> for Line<'a> {
    fn from(value: &'a str) -> Self {
        Self::from(Span::raw(value))
    }
}

impl From<String> for Line<'_> {
    fn from(value: String) -> Self {
        Self::from(Span::raw(value))
    }
}

impl<'a> From<Span<'a>> for Line<'a> {
    fn from(span: Span<'a>) -> Self {
        Self {
            spans: vec![span],
            style: Style::default(),
            alignment: None,
        }
    }
}

impl<'a> From<Vec<Span<'a>>> for Line<'a> {
    fn from(spans: Vec<Span<'a>>) -> Self {
        Self {
            spans,
            style: Style::default(),
            alignment: None,
        }
    }
}

impl fmt::Display for Line<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for span in &self.spans {
            formatter.write_str(&span.content)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Text<'a> {
    pub lines: Vec<Line<'a>>,
    pub style: Style,
    pub alignment: Option<Alignment>,
}

impl<'a> From<&'a str> for Text<'a> {
    fn from(value: &'a str) -> Self {
        Self {
            lines: value.split('\n').map(Line::from).collect(),
            style: Style::default(),
            alignment: None,
        }
    }
}

impl From<String> for Text<'_> {
    fn from(value: String) -> Self {
        Self {
            lines: value
                .split('\n')
                .map(|line| Line::from(line.to_string()))
                .collect(),
            style: Style::default(),
            alignment: None,
        }
    }
}

impl<'a> From<Line<'a>> for Text<'a> {
    fn from(line: Line<'a>) -> Self {
        Self {
            lines: vec![line],
            style: Style::default(),
            alignment: None,
        }
    }
}

impl<'a> From<Vec<Line<'a>>> for Text<'a> {
    fn from(lines: Vec<Line<'a>>) -> Self {
        Self {
            lines,
            style: Style::default(),
            alignment: None,
        }
    }
}

impl<'a> From<Span<'a>> for Text<'a> {
    fn from(span: Span<'a>) -> Self {
        Self::from(Line::from(span))
    }
}
