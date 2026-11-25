//! Steady-state cost of drawing the whole window over the fixture session.
//!
//! Run with `cargo bench -p soulseek-rs --features fixtures --bench tui_frame`.

use soulseek_rs_tui::{FixtureSession, MainTui, Terminal, TestBackend};
use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

const WIDTH: u16 = 120;
const HEIGHT: u16 = 40;
const WARMUP: usize = 64;
const SAMPLES: usize = 21;
const ITERATIONS: usize = 256;

fn main() {
    let client = Arc::new(FixtureSession::sample());
    let mut tui = MainTui::new(
        client,
        "/home/tester/Music".to_string(),
        false,
        Duration::from_secs(60),
        None,
        None,
    );
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("test backend");

    for _ in 0..WARMUP {
        black_box(draw(&mut terminal, &mut tui));
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        for _ in 0..ITERATIONS {
            black_box(draw(&mut terminal, &mut tui));
        }
        samples.push(
            start.elapsed().as_secs_f64() * 1_000_000_000.0
                / f64::from(u32::try_from(ITERATIONS).expect("iterations fit")),
        );
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "tui_frame: {:.0} ns/iter ({WIDTH}x{HEIGHT}, {ITERATIONS} iterations, \
         median of {SAMPLES} batches)",
        samples[SAMPLES / 2]
    );
}

fn draw(terminal: &mut Terminal<TestBackend>, tui: &mut MainTui) -> usize {
    terminal
        .draw(|frame| tui.render(frame))
        .expect("drawing the fixture window");
    terminal.backend().buffer().content.len()
}
