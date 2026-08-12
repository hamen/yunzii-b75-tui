//! The interactive interface.
//!
//! Running the binary with no subcommand lands here. Every subcommand keeps
//! working exactly as before, so nothing that used to work changes meaning --
//! the subcommand was previously required, so no existing invocation is
//! affected.

pub mod app;
pub mod preview;
pub mod term;
pub mod ui;
pub mod worker;

use app::{App, Key};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long to wait for a keypress before redrawing anyway.
///
/// The redraw is what animates the progress bar's remaining-time estimate
/// while the worker is busy, so it wants to be short enough to look alive and
/// long enough not to spin.
const TICK: Duration = Duration::from_millis(100);

/// Whether this process is attached to a terminal on both ends.
///
/// Checked because launching a full-screen interface into a pipe produces a
/// program that appears to hang: it draws to nowhere and waits for keys that
/// will never come. A tool that hangs in a script is a worse regression than
/// the usage error it replaced.
pub fn is_interactive() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// How long a confirmed quit waits for the worker to stop.
///
/// The plan's number. Long enough for a report and its acknowledgement, short
/// enough that a thread which has wedged does not hold the terminal hostage.
const CANCEL_GRACE: Duration = Duration::from_secs(2);

pub fn run() -> io::Result<()> {
    let guard = term::Guard::new(term::RealTerm)?;
    // The hook shares the guard's flag, so whichever runs first wins and the
    // other does nothing. Without that they both restore, and leaving the
    // alternate screen twice pops one the user was already in.
    term::install_panic_hook(guard.flag());

    let result = event_loop();
    guard.restore();

    // After the alternate screen is gone, so it can actually be read.
    match result {
        Ok(Some(message)) => {
            eprintln!("{message}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Returns a message to print once the terminal is back, if there is one.
fn event_loop() -> io::Result<Option<String>> {
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::CrosstermBackend::new(io::stdout()))?;

    let start_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let mut app = App::new(start_dir);

    let mut quit_deadline: Option<Instant> = None;
    let (job_tx, job_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    let flags = worker::Flags::default();

    worker::spawn_worker(
        job_rx,
        update_tx.clone(),
        flags.busy.clone(),
        flags.ready.clone(),
    );
    worker::spawn_discovery(update_tx.clone(), flags.busy.clone(), flags.ready.clone());

    loop {
        terminal.draw(|f| ui::draw(f, &app))?;

        // Everything the background threads have said since the last draw.
        loop {
            match update_rx.try_recv() {
                Ok(u) => app.on_update(u),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Every sender is gone. In practice the discovery thread
                    // holds one for the life of the program, so this is a
                    // backstop rather than the main way a dead worker is
                    // noticed -- that is `guarded()` in worker.rs, which turns
                    // a panicking job into a message and keeps the thread.
                    app.final_message = Some("the background threads stopped unexpectedly.".into());
                    app.should_quit = true;
                    break;
                }
            }
        }

        // A confirmed quit waits for the worker to acknowledge the cancel,
        // but not forever.
        match (app.quitting, quit_deadline) {
            (true, None) => quit_deadline = Some(Instant::now() + CANCEL_GRACE),
            (true, Some(t)) if Instant::now() >= t => app.give_up_waiting(),
            _ => {}
        }

        if app.should_quit {
            break;
        }

        if event::poll(TICK)?
            && let Event::Key(k) = event::read()?
            && k.kind == KeyEventKind::Press
            && let Some(key) = translate(k.code)
            && let Some(job) = app.on_key(key)
            && job_tx.send(job).is_err()
        {
            // The worker is gone. Leaving silently would look like the key did
            // nothing; say so on the normal screen instead.
            app.final_message = Some("the background worker stopped, so nothing was sent.".into());
            break;
        }
    }

    Ok(app.final_message)
}

fn translate(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Char(c) => Key::Char(c),
        _ => return None,
    })
}
