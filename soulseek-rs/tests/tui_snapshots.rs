//! Frozen terminal frames of the whole window over a deterministic session.
//!
//! The fixture binary renders the real `MainTui`, enters the same raw mode and
//! alternate screen, and turns mouse reporting on. The frames below are the
//! oracle the hand-rolled terminal layer has to match.
//!
//! Re-record only after an intended UI change:
//! `PTYTEST_UPDATE_SNAPSHOTS=1 cargo test --features fixtures --test tui_snapshots`

#![cfg(feature = "fixtures")]

use ptytest::{
    CommandSpec, ExitStatus, Key, PtyTest, Scenario, Size, SnapshotOptions,
    TerminalBaseline, TestEnv,
};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

const STEP: Duration = Duration::from_secs(10);

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.ptytest"))
}

fn start(label: &str, columns: u16, rows: u16) -> (PtyTest, TerminalBaseline) {
    let scenario = Scenario::new(label)
        .expect("scenario")
        .command(CommandSpec::new(env!("CARGO_BIN_EXE_tui-fixture")))
        .size(Size::new(columns, rows).expect("size"))
        .environment(TestEnv::hermetic().expect("environment"));
    let terminal = PtyTest::spawn(scenario).expect("spawn fixture");
    let baseline = terminal.terminal_baseline();
    (terminal, baseline)
}

fn settle(terminal: &mut PtyTest) {
    let deadline = terminal.deadline(STEP);
    assert!(
        terminal
            .wait_for_quiescence(deadline, Duration::from_millis(40))
            .expect("quiescence"),
        "the fixture did not settle"
    );
}

fn wait_for(terminal: &mut PtyTest, text: &str) {
    let deadline = terminal.deadline(STEP);
    terminal
        .wait_for_screen(deadline, text, |screen| screen.contains(text))
        .unwrap_or_else(|error| panic!("{text}: {error}"));
    settle(terminal);
}

fn wait_until_absent(terminal: &mut PtyTest, text: &str) {
    let deadline = terminal.deadline(STEP);
    terminal
        .wait_for_screen(deadline, format!("{text} to disappear"), |screen| {
            !screen.contains(text)
        })
        .unwrap_or_else(|error| panic!("{text} never disappeared: {error}"));
    settle(terminal);
}

fn press(terminal: &mut PtyTest, key: Key) {
    let deadline = terminal.deadline(STEP);
    terminal.send_key(deadline, key).expect("send key");
}

fn type_text(terminal: &mut PtyTest, text: &str) {
    let deadline = terminal.deadline(STEP);
    terminal.send_text(deadline, text).expect("send text");
}

fn freeze(terminal: &PtyTest, name: &str) {
    let actual = normalize_snapshot(
        &terminal
            .screen()
            .to_text(SnapshotOptions::default().with_attributes()),
    );
    let path = golden(name);
    if std::env::var("PTYTEST_UPDATE_SNAPSHOTS").is_ok_and(|value| value == "1")
    {
        fs::write(&path, actual).expect("write snapshot");
        return;
    }
    let expected = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    assert_eq!(expected, actual, "snapshot {name} differs");
}

/// The hidden cursor is not visible, and the position left behind by a
/// different diff strategy cannot be seen. Room messages also carry the wall
/// clock when the fixture delivers them, and a spinner frame depends on when
/// the capture lands.
fn normalize_snapshot(snapshot: &str) -> String {
    let mut normalized = String::with_capacity(snapshot.len());
    for line in snapshot.lines() {
        if line.starts_with("cursor:") && line.contains("visible=false") {
            normalized.push_str("cursor: hidden\n");
        } else {
            normalized.push_str(&normalize_spinner(&normalize_clock(line)));
            normalized.push('\n');
        }
    }
    normalized
}

fn normalize_spinner(snapshot: &str) -> String {
    snapshot
        .chars()
        .map(|character| {
            if "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏".contains(character) {
                '⠋'
            } else {
                character
            }
        })
        .collect()
}

fn normalize_clock(snapshot: &str) -> String {
    let chars: Vec<char> = snapshot.chars().collect();
    let mut output = String::with_capacity(snapshot.len());
    let mut index = 0;
    while index < chars.len() {
        let clock = index + 5 <= chars.len()
            && chars[index].is_ascii_digit()
            && chars[index + 1].is_ascii_digit()
            && chars[index + 2] == ':'
            && chars[index + 3].is_ascii_digit()
            && chars[index + 4].is_ascii_digit()
            && (index == 0 || !chars[index - 1].is_ascii_digit())
            && (index + 5 == chars.len() || !chars[index + 5].is_ascii_digit());
        if clock {
            output.push_str("<time>");
            index += 5;
        } else {
            output.push(chars[index]);
            index += 1;
        }
    }
    output
}

fn quit(terminal: &mut PtyTest, baseline: &TerminalBaseline) {
    type_text(terminal, "q");
    let deadline = terminal.deadline(STEP);
    assert_eq!(
        terminal.wait_for_exit(deadline).expect("exit"),
        ExitStatus::Code(0)
    );
    terminal
        .assert_terminal_restored(baseline)
        .expect("terminal restored");
    let deadline = terminal.deadline(STEP);
    assert_eq!(
        terminal.finish(deadline).expect("reap"),
        ExitStatus::Code(0)
    );
}

#[test]
fn login_screen_matches_the_recorded_frame() {
    let scenario = Scenario::new("tui-login")
        .expect("scenario")
        .command(CommandSpec::new(env!("CARGO_BIN_EXE_soulseek-rs")))
        .size(Size::new(80, 24).expect("size"))
        .environment(TestEnv::hermetic().expect("environment"));
    let mut terminal = PtyTest::spawn(scenario).expect("spawn program");

    wait_for(&mut terminal, "Username:");
    freeze(&terminal, "login-80x24");

    press(&mut terminal, Key::Escape);
    let deadline = terminal.deadline(STEP);
    assert_eq!(
        terminal.wait_for_exit(deadline).expect("exit"),
        ExitStatus::Code(0)
    );
    let deadline = terminal.deadline(STEP);
    assert_eq!(
        terminal.finish(deadline).expect("reap"),
        ExitStatus::Code(0)
    );
}

#[test]
fn populated_window_matches_the_recorded_frames() {
    let (mut terminal, baseline) = start("tui-populated", 120, 40);

    wait_for(&mut terminal, "aphex twi");
    wait_for(&mut terminal, "Downloads/Uploads");
    freeze(&terminal, "main-120x40");

    // The restored, completed search brings its results into the pane.
    press(&mut terminal, Key::Down);
    press(&mut terminal, Key::Enter);
    wait_for(&mut terminal, "Pulsewidth");
    freeze(&terminal, "results-120x40");

    type_text(&mut terminal, "?");
    wait_for(&mut terminal, "Keys");
    freeze(&terminal, "help-120x40");
    type_text(&mut terminal, "?");
    wait_until_absent(&mut terminal, "Keys");

    type_text(&mut terminal, "i");
    wait_for(&mut terminal, "idm_peer");
    wait_for(&mut terminal, "do you have the remastered Tha too?");
    freeze(&terminal, "chat-120x40");
    press(&mut terminal, Key::Escape);
    wait_until_absent(&mut terminal, "Chats (");

    type_text(&mut terminal, "c");
    wait_for(&mut terminal, "The Ambient Room");
    freeze(&terminal, "rooms-list-120x40");
    press(&mut terminal, Key::Enter);
    wait_for(&mut terminal, "anyone else on the new Autechre?");
    freeze(&terminal, "rooms-chat-120x40");
    press(&mut terminal, Key::Escape);
    wait_until_absent(&mut terminal, "anyone else on the new Autechre?");
    press(&mut terminal, Key::Escape);
    wait_until_absent(&mut terminal, "The Ambient Room");

    type_text(&mut terminal, "o");
    wait_for(&mut terminal, "Signed in as");
    freeze(&terminal, "settings-120x40");
    press(&mut terminal, Key::Escape);
    wait_until_absent(&mut terminal, "Signed in as");

    type_text(&mut terminal, "b");
    type_text(&mut terminal, "idm_peer");
    press(&mut terminal, Key::Enter);
    wait_for(&mut terminal, "Browse idm_peer");
    freeze(&terminal, "browse-120x40");
    press(&mut terminal, Key::Escape);
    wait_until_absent(&mut terminal, "Browse idm_peer");

    type_text(&mut terminal, "s");
    type_text(&mut terminal, "boards of canada");
    wait_for(&mut terminal, "boards of canada");
    freeze(&terminal, "command-bar-120x40");
    press(&mut terminal, Key::Escape);
    wait_until_absent(&mut terminal, "boards of canada");

    quit(&mut terminal, &baseline);
}

#[test]
fn narrow_window_still_fits() {
    let (mut terminal, baseline) = start("tui-narrow", 40, 24);

    wait_for(&mut terminal, "Downloads/Up");
    freeze(&terminal, "main-40x24");

    press(&mut terminal, Key::Down);
    press(&mut terminal, Key::Enter);
    wait_for(&mut terminal, "Results: aphex");
    freeze(&terminal, "results-40x24");

    quit(&mut terminal, &baseline);
}

#[test]
fn tiny_window_renders_without_content_rows() {
    let (mut terminal, baseline) = start("tui-tiny", 40, 12);

    wait_for(&mut terminal, "Status");
    freeze(&terminal, "main-40x12");

    quit(&mut terminal, &baseline);
}

#[test]
fn wide_window_keeps_the_columns() {
    let (mut terminal, baseline) = start("tui-wide", 200, 60);

    wait_for(&mut terminal, "aphex twi");
    freeze(&terminal, "main-200x60");
    press(&mut terminal, Key::Down);
    press(&mut terminal, Key::Enter);
    wait_for(&mut terminal, "Pulsewidth");
    freeze(&terminal, "results-200x60");

    quit(&mut terminal, &baseline);
}
