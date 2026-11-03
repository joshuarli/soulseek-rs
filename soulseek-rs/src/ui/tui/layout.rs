//! Geometry and the constraint layout used by the renderer.

use std::collections::HashMap;
use std::iter::repeat_with;

use kasuari::WeightedRelation::{EQ, GE, LE};
use kasuari::{Expression, Solver, Strength, Variable};

const FLOAT_PRECISION_MULTIPLIER: f64 = 100.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    #[must_use]
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    #[must_use]
    pub const fn area(self) -> u32 {
        self.width as u32 * self.height as u32
    }

    #[must_use]
    pub const fn left(self) -> u16 {
        self.x
    }

    #[must_use]
    pub const fn top(self) -> u16 {
        self.y
    }

    #[must_use]
    pub const fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    #[must_use]
    pub const fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    #[must_use]
    pub const fn inner(self, margin: Margin) -> Self {
        Self {
            x: self.x.saturating_add(margin.horizontal),
            y: self.y.saturating_add(margin.vertical),
            width: self
                .width
                .saturating_sub(margin.horizontal.saturating_mul(2)),
            height: self
                .height
                .saturating_sub(margin.vertical.saturating_mul(2)),
        }
    }

    #[must_use]
    pub fn intersection(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        Self {
            x,
            y,
            width: right.saturating_sub(x),
            height: bottom.saturating_sub(y),
        }
    }

    #[must_use]
    pub const fn contains(self, position: Position) -> bool {
        position.x >= self.x
            && position.x < self.right()
            && position.y >= self.y
            && position.y < self.bottom()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Position {
    pub x: u16,
    pub y: u16,
}

impl Position {
    #[must_use]
    pub const fn new(x: u16, y: u16) -> Self {
        Self { x, y }
    }

    pub const ORIGIN: Self = Self::new(0, 0);
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Size {
    pub width: u16,
    pub height: u16,
}

impl Size {
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Self {
        Self { width, height }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Margin {
    pub horizontal: u16,
    pub vertical: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Alignment {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Flex {
    #[default]
    Start,
    Center,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Constraint {
    Length(u16),
    Percentage(u16),
    Fill(u16),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Direction {
    Horizontal,
    #[default]
    Vertical,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    direction: Direction,
    constraints: Vec<Constraint>,
    spacing: u16,
    flex: Flex,
}

impl Layout {
    #[must_use]
    pub fn vertical(constraints: impl Into<Vec<Constraint>>) -> Self {
        Self {
            direction: Direction::Vertical,
            constraints: constraints.into(),
            spacing: 0,
            flex: Flex::default(),
        }
    }

    #[must_use]
    pub fn horizontal(constraints: impl Into<Vec<Constraint>>) -> Self {
        Self {
            direction: Direction::Horizontal,
            constraints: constraints.into(),
            spacing: 0,
            flex: Flex::default(),
        }
    }

    #[must_use]
    pub const fn spacing(mut self, spacing: u16) -> Self {
        self.spacing = spacing;
        self
    }

    #[must_use]
    pub const fn flex(mut self, flex: Flex) -> Self {
        self.flex = flex;
        self
    }

    #[must_use]
    pub fn split(&self, area: Rect) -> Vec<Rect> {
        self.try_split(area)
            .expect("layout constraints must be satisfiable")
    }

    fn try_split(&self, area: Rect) -> Result<Vec<Rect>, kasuari::AddConstraintError> {
        let mut solver = Solver::new();
        let (area_start, area_end) = match self.direction {
            Direction::Horizontal => (
                f64::from(area.x) * FLOAT_PRECISION_MULTIPLIER,
                f64::from(area.right()) * FLOAT_PRECISION_MULTIPLIER,
            ),
            Direction::Vertical => (
                f64::from(area.y) * FLOAT_PRECISION_MULTIPLIER,
                f64::from(area.bottom()) * FLOAT_PRECISION_MULTIPLIER,
            ),
        };

        let variable_count = self.constraints.len() * 2 + 2;
        let variables = repeat_with(Variable::new)
            .take(variable_count)
            .collect::<Vec<_>>();
        let spacers = variables
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| Element::from(*pair))
            .collect::<Vec<_>>();
        let segments = variables[1..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| Element::from(*pair))
            .collect::<Vec<_>>();

        let flex = self.flex;
        let spacing = i16::try_from(self.spacing).unwrap_or(i16::MAX);
        let area_element =
            Element::from((variables[0], variables[variable_count - 1]));

        configure_area(&mut solver, area_element, area_start, area_end)?;
        configure_variable_in_area_constraints(
            &mut solver,
            &variables,
            area_element,
        )?;
        configure_variable_constraints(&mut solver, &variables)?;
        configure_flex_constraints(
            &mut solver,
            area_element,
            &spacers,
            flex,
            spacing,
        )?;
        configure_constraints(
            &mut solver,
            area_element,
            &segments,
            &self.constraints,
        )?;
        configure_fill_constraints(
            &mut solver,
            &segments,
            &self.constraints,
        )?;
        for pair in segments.windows(2) {
            solver.add_constraint(pair[0].has_size(pair[1], ALL_SEGMENT_GROW))?;
        }

        let changes: HashMap<Variable, f64> =
            solver.fetch_changes().iter().copied().collect();
        Ok(changes_to_rects(
            &changes,
            &segments,
            area,
            self.direction,
        ))
    }

    #[must_use]
    pub fn areas<const N: usize>(&self, area: Rect) -> [Rect; N] {
        let areas = self.split(area);
        areas
            .into_iter()
            .collect::<Vec<_>>()
            .try_into()
            .unwrap_or_else(|areas: Vec<Rect>| {
                panic!("expected {N} areas, got {}", areas.len())
            })
    }
}

fn configure_area(
    solver: &mut Solver,
    area: Element,
    area_start: f64,
    area_end: f64,
) -> Result<(), kasuari::AddConstraintError> {
    solver.add_constraint(area.start | EQ(Strength::REQUIRED) | area_start)?;
    solver.add_constraint(area.end | EQ(Strength::REQUIRED) | area_end)?;
    Ok(())
}

fn configure_variable_in_area_constraints(
    solver: &mut Solver,
    variables: &[Variable],
    area: Element,
) -> Result<(), kasuari::AddConstraintError> {
    for &variable in variables {
        solver.add_constraint(variable | GE(Strength::REQUIRED) | area.start)?;
        solver.add_constraint(variable | LE(Strength::REQUIRED) | area.end)?;
    }
    Ok(())
}

fn configure_variable_constraints(
    solver: &mut Solver,
    variables: &[Variable],
) -> Result<(), kasuari::AddConstraintError> {
    for pair in variables[1..].as_chunks::<2>().0 {
        solver.add_constraint(pair[0] | LE(Strength::REQUIRED) | pair[1])?;
    }
    Ok(())
}

fn configure_constraints(
    solver: &mut Solver,
    area: Element,
    segments: &[Element],
    constraints: &[Constraint],
) -> Result<(), kasuari::AddConstraintError> {
    for (&constraint, &segment) in constraints.iter().zip(segments) {
        match constraint {
            Constraint::Length(length) => {
                solver.add_constraint(
                    segment.has_int_size(length, LENGTH_SIZE_EQ),
                )?;
            }
            Constraint::Percentage(percent) => {
                let size =
                    area.size() * f64::from(percent) / 100.00;
                solver.add_constraint(segment.has_size(size, PERCENTAGE_SIZE_EQ))?;
            }
            Constraint::Fill(_) => {
                solver.add_constraint(segment.has_size(area, FILL_GROW))?;
            }
        }
    }
    Ok(())
}

fn configure_flex_constraints(
    solver: &mut Solver,
    area: Element,
    spacers: &[Element],
    flex: Flex,
    spacing: i16,
) -> Result<(), kasuari::AddConstraintError> {
    let middle = spacers
        .get(1..spacers.len().saturating_sub(1))
        .unwrap_or(&[]);
    let spacing_f64 = f64::from(spacing) * FLOAT_PRECISION_MULTIPLIER;
    match flex {
        Flex::Start => {
            for spacer in middle {
                solver.add_constraint(
                    spacer.has_size(spacing_f64, SPACER_SIZE_EQ),
                )?;
            }
            if let (Some(first), Some(last)) = (spacers.first(), spacers.last())
            {
                solver.add_constraint(first.is_empty())?;
                solver.add_constraint(last.has_size(area, GROW))?;
            }
        }
        Flex::Center => {
            for spacer in middle {
                solver.add_constraint(
                    spacer.has_size(spacing_f64, SPACER_SIZE_EQ),
                )?;
            }
            if let (Some(first), Some(last)) = (spacers.first(), spacers.last())
            {
                solver.add_constraint(first.has_size(area, GROW))?;
                solver.add_constraint(last.has_size(area, GROW))?;
                solver.add_constraint(first.has_size(last, SPACER_SIZE_EQ))?;
            }
        }
    }
    Ok(())
}

fn configure_fill_constraints(
    solver: &mut Solver,
    segments: &[Element],
    constraints: &[Constraint],
) -> Result<(), kasuari::AddConstraintError> {
    let fills = constraints
        .iter()
        .zip(segments)
        .filter(|(constraint, _)| matches!(constraint, Constraint::Fill(_)))
        .collect::<Vec<_>>();
    for (index, (left_constraint, left_segment)) in fills.iter().enumerate() {
        for (right_constraint, right_segment) in &fills[index + 1..] {
            let Constraint::Fill(left_scale) = left_constraint else {
                unreachable!()
            };
            let Constraint::Fill(right_scale) = right_constraint else {
                unreachable!()
            };
            let left_scaling_factor = f64::from(*left_scale).max(1e-6);
            let right_scaling_factor = f64::from(*right_scale).max(1e-6);
            solver.add_constraint(
                (right_scaling_factor * left_segment.size())
                    | EQ(GROW)
                    | (left_scaling_factor * right_segment.size()),
            )?;
        }
    }
    Ok(())
}

fn changes_to_rects(
    changes: &HashMap<Variable, f64>,
    elements: &[Element],
    area: Rect,
    direction: Direction,
) -> Vec<Rect> {
    elements
        .iter()
        .map(|element| {
            let start = changes.get(&element.start).unwrap_or(&0.0);
            let end = changes.get(&element.end).unwrap_or(&0.0);
            let start =
                round(round(*start) / FLOAT_PRECISION_MULTIPLIER) as u16;
            let end = round(round(*end) / FLOAT_PRECISION_MULTIPLIER) as u16;
            let size = end.saturating_sub(start);
            match direction {
                Direction::Horizontal => {
                    Rect::new(start, area.y, size, area.height)
                }
                Direction::Vertical => {
                    Rect::new(area.x, start, area.width, size)
                }
            }
        })
        .collect()
}

const fn round(value: f64) -> f64 {
    value.round()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct Element {
    start: Variable,
    end: Variable,
}

impl From<(Variable, Variable)> for Element {
    fn from((start, end): (Variable, Variable)) -> Self {
        Self { start, end }
    }
}

impl From<[Variable; 2]> for Element {
    fn from([start, end]: [Variable; 2]) -> Self {
        Self { start, end }
    }
}

impl Element {
    fn size(self) -> Expression {
        self.end - self.start
    }

    fn has_int_size(
        self,
        size: u16,
        strength: Strength,
    ) -> kasuari::Constraint {
        self.size() | EQ(strength) | (f64::from(size) * FLOAT_PRECISION_MULTIPLIER)
    }

    fn has_size(
        self,
        size: impl Into<Expression>,
        strength: Strength,
    ) -> kasuari::Constraint {
        self.size() | EQ(strength) | size.into()
    }

    fn is_empty(self) -> kasuari::Constraint {
        self.size() | EQ(Strength::REQUIRED - Strength::WEAK) | 0.0
    }
}

impl From<Element> for Expression {
    fn from(element: Element) -> Self {
        element.size()
    }
}

impl From<&Element> for Expression {
    fn from(element: &Element) -> Self {
        element.size()
    }
}

mod strengths {
    use kasuari::Strength;

    pub const SPACER_SIZE_EQ: Strength = Strength::REQUIRED.div_f64(10.0);
    pub const LENGTH_SIZE_EQ: Strength = Strength::STRONG.mul_f64(10.0);
    pub const PERCENTAGE_SIZE_EQ: Strength = Strength::STRONG;
    pub const FILL_GROW: Strength = Strength::MEDIUM;
    pub const GROW: Strength = Strength::MEDIUM.div_f64(10.0);
    pub const ALL_SEGMENT_GROW: Strength = Strength::WEAK;
}

use strengths::{
    ALL_SEGMENT_GROW, FILL_GROW, GROW, LENGTH_SIZE_EQ, PERCENTAGE_SIZE_EQ,
    SPACER_SIZE_EQ,
};
