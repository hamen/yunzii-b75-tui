//! What the interface *is*, separated from how it is drawn.
//!
//! Everything here is a plain struct and two functions over it: `on_key` and
//! `on_update`. No terminal, no device, no threads. That is what makes the
//! interesting behaviour -- disabled actions, the confirm step, cancelling,
//! quitting mid-upload -- ordinary unit tests rather than something only a
//! person with a keyboard plugged in can check.

use crate::adjust::Adjustments;
use crate::plan::{self, GifPlan, PicturePlan};
use crate::protocol::Page;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// What the discovery thread last found.
///
/// Five states rather than found/not-found, because `find_device` and the
/// first `drain` fail in genuinely different ways and the fix differs each
/// time. Telling someone "no keyboard" when the real answer is "you are not in
/// the plugdev group" wastes their afternoon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceState {
    Ready(PathBuf),
    NotFound,
    /// The node exists but cannot be opened or read.
    PermissionDenied(PathBuf),
    /// Two keyboards match; picking one would be a guess.
    MultipleMatches(Vec<PathBuf>),
    /// Discovery itself failed -- an unreadable /sys, say.
    ScanFailed(String),
}

impl DeviceState {
    pub fn is_ready(&self) -> bool {
        matches!(self, DeviceState::Ready(_))
    }

    /// One line for the header, saying what to do about it.
    pub fn summary(&self) -> String {
        match self {
            DeviceState::Ready(p) => p.display().to_string(),
            DeviceState::NotFound => "no keyboard found -- rescanning".into(),
            DeviceState::PermissionDenied(p) => {
                format!(
                    "{} -- permission denied; install the udev rule (README)",
                    p.display()
                )
            }
            DeviceState::MultipleMatches(paths) => {
                format!("{} matching devices -- refusing to guess", paths.len())
            }
            DeviceState::ScanFailed(e) => format!("scan failed: {e} -- rescanning"),
        }
    }
}

/// The actions in the menu, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    SetTime,
    ShowHome,
    ShowPicture,
    ShowGif,
    ClearPicture,
    UploadPicture,
    UploadGif,
}

impl Action {
    pub const ALL: [Action; 7] = [
        Action::SetTime,
        Action::ShowHome,
        Action::ShowPicture,
        Action::ShowGif,
        Action::ClearPicture,
        Action::UploadPicture,
        Action::UploadGif,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::SetTime => "Set time",
            Action::ShowHome => "Show home page",
            Action::ShowPicture => "Show picture page",
            Action::ShowGif => "Show GIF page",
            Action::ClearPicture => "Clear picture",
            Action::UploadPicture => "Upload picture…",
            Action::UploadGif => "Upload GIF…",
        }
    }
}

/// What the interface is doing right now.
#[derive(Debug)]
pub enum Screen {
    Menu,
    /// Picking a file for an upload.
    Browse {
        for_gif: bool,
        dir: PathBuf,
        entries: Vec<Entry>,
        selected: usize,
        error: Option<String>,
    },
    /// A file has been decoded and is waiting for a second, explicit keypress.
    ///
    /// Nothing reaches the keyboard until then. The plan the preview was built
    /// from is the plan that gets uploaded, so what you looked at is what you
    /// send.
    Confirm(Box<Pending>),
    Running(Running),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug)]
pub enum Pending {
    Picture {
        path: PathBuf,
        plan: PicturePlan,
        adjustments: Adjustments,
        /// Which row of the confirm list is selected.
        row: usize,
    },
    Gif {
        path: PathBuf,
        plan: GifPlan,
        /// `None` means "as the file asks"; `Some` is the equivalent of
        /// passing `--fps`, and suppresses the fallback note the same way.
        rate_override: Option<u8>,
        adjustments: Adjustments,
        row: usize,
    },
}

/// One line of the confirm screen.
///
/// The rate is a row like any other, which is what lets the arrow keys keep
/// meaning what they meant before adjustments existed: `Rate` is row 0, so
/// left and right still change the frame rate until you move off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Rate,
    Brightness,
    Chroma,
    Saturation,
    Grayscale,
    Sharpen,
    Blur,
}

impl Row {
    pub const GIF: [Row; 7] = [
        Row::Rate,
        Row::Brightness,
        Row::Chroma,
        Row::Saturation,
        Row::Grayscale,
        Row::Sharpen,
        Row::Blur,
    ];
    /// A picture has no frame rate.
    pub const PICTURE: [Row; 6] = [
        Row::Brightness,
        Row::Chroma,
        Row::Saturation,
        Row::Grayscale,
        Row::Sharpen,
        Row::Blur,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Row::Rate => "Rate",
            Row::Brightness => "Brightness",
            Row::Chroma => "Chroma",
            Row::Saturation => "Saturation",
            Row::Grayscale => "Grayscale",
            Row::Sharpen => "Sharpen",
            Row::Blur => "Blur",
        }
    }
}

/// How far one arrow press moves a slider. The vendor's step.
const ADJUST_STEP: f64 = 0.05;

impl Pending {
    pub fn rows(&self) -> &'static [Row] {
        match self {
            Pending::Picture { .. } => &Row::PICTURE,
            Pending::Gif { .. } => &Row::GIF,
        }
    }

    pub fn row(&self) -> usize {
        match self {
            Pending::Picture { row, .. } | Pending::Gif { row, .. } => *row,
        }
    }

    pub fn adjustments(&self) -> &Adjustments {
        match self {
            Pending::Picture { adjustments, .. } | Pending::Gif { adjustments, .. } => adjustments,
        }
    }

    /// Re-encodes every frame from the pristine panel pixels.
    ///
    /// Always from the originals, never on top of the last result, so moving a
    /// slider back and forth returns exactly where it started instead of
    /// accumulating rounding.
    pub fn reencode_all(&mut self) {
        match self {
            Pending::Picture {
                plan, adjustments, ..
            } => {
                plan.pixels = plan::adjust_and_encode(&plan.panel_rgba, adjustments);
            }
            Pending::Gif {
                plan, adjustments, ..
            } => {
                for (out, src) in plan.frames.iter_mut().zip(plan.panel_rgba.iter()) {
                    *out = plan::adjust_and_encode(src, adjustments);
                }
            }
        }
    }

    /// Re-encodes only the frame the preview shows.
    ///
    /// What a keypress runs: 15,360 pixels through at most six filters, which
    /// is fast enough on the drawing thread. The whole set is only recomputed
    /// when the upload starts, on the worker.
    pub fn reencode_preview(&mut self) {
        match self {
            Pending::Picture { .. } => self.reencode_all(),
            Pending::Gif {
                plan, adjustments, ..
            } => {
                if let (Some(out), Some(src)) = (plan.frames.first_mut(), plan.panel_rgba.first()) {
                    *out = plan::adjust_and_encode(src, adjustments);
                }
            }
        }
    }

    /// The panel frame this will send, for the preview to draw.
    ///
    /// Kept as pixels rather than a pre-rendered `Preview`: the pane knows how
    /// much room it has, and rendering once at that size beats rendering large
    /// and squeezing the cells afterwards, which resamples twice and loses
    /// detail for no reason.
    pub fn frame(&self) -> &[u8] {
        match self {
            Pending::Picture { plan, .. } => &plan.pixels,
            Pending::Gif { plan, .. } => plan.frames.first().map_or(&[][..], |f| &f[..]),
        }
    }
}

#[derive(Debug)]
pub struct Running {
    pub label: String,
    pub done: usize,
    pub total: usize,
    pub frame: Option<(usize, usize)>,
    pub started: Instant,
    pub est_secs: Option<usize>,
    pub cancel: Arc<AtomicBool>,
    pub cancelling: bool,
}

impl Running {
    /// Seconds left, from measured rate once there is enough to measure.
    ///
    /// The planner's estimate is used until then: for the first few reports the
    /// measured rate is noise, and a countdown that starts at four hours is
    /// worse than no countdown.
    pub fn remaining_secs(&self) -> Option<usize> {
        if self.done < 16 || self.total == 0 {
            return self.est_secs;
        }
        let elapsed = self.started.elapsed().as_secs_f64();
        let per_report = elapsed / self.done as f64;
        Some(((self.total - self.done) as f64 * per_report).round() as usize)
    }

    pub fn percent(&self) -> u16 {
        if self.total == 0 {
            return 0;
        }
        ((self.done * 100) / self.total).min(100) as u16
    }
}

/// A job for the worker thread.
#[derive(Debug)]
pub enum Job {
    Preview {
        path: PathBuf,
        for_gif: bool,
        /// Which request this is. A decode takes a moment; if the user leaves
        /// the browser meanwhile, the answer must be dropped rather than
        /// yanking them into a confirm screen they already dismissed.
        generation: u64,
    },
    SetTime,
    SwitchPage(Page),
    ClearPicture,
    UploadPicture(Box<PicturePlan>),
    UploadGif(Box<GifPlan>),
}

/// Something that happened elsewhere.
#[derive(Debug)]
pub enum Update {
    Device(DeviceState),
    Preview {
        generation: u64,
        result: Box<Result<Pending, String>>,
    },
    Started {
        label: String,
        total: usize,
        est_secs: Option<usize>,
        cancel: Arc<AtomicBool>,
    },
    Progress {
        done: usize,
        total: usize,
        frame: Option<(usize, usize)>,
    },
    Note(String),
    Finished(Result<String, String>),
}

/// A key, reduced to what this interface cares about.
///
/// Its own type so the state machine does not depend on crossterm, and so a
/// test says `Key::Down` instead of constructing a terminal event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Enter,
    Esc,
    Backspace,
    Char(char),
}

pub struct App {
    /// Where the file browser opens. Remembered between visits, so picking a
    /// second file does not start from the top again.
    pub browse_dir: PathBuf,
    pub device: DeviceState,
    pub screen: Screen,
    pub selected: usize,
    pub log: Vec<String>,
    pub status: Option<String>,
    /// Set when the user asked to quit during a job and has not answered yet.
    pub quit_confirm: bool,
    /// The user confirmed a quit and the worker is being asked to stop.
    ///
    /// Quitting does not happen here: leaving while a report is in flight is
    /// how you get a half-written animation *and* no message about it. The
    /// event loop waits for the worker to acknowledge, with a deadline.
    pub quitting: bool,
    pub should_quit: bool,
    /// Printed on the normal screen after the terminal is restored.
    ///
    /// A partial-write warning shown inside the alternate screen is a warning
    /// nobody reads: the screen is torn down a moment later.
    pub final_message: Option<String>,
    /// Bumped for every preview request, and again whenever the browser is
    /// left, so a reply that arrives too late can be recognised.
    pub preview_generation: u64,
    /// A job has been handed to the worker and has not reported back.
    ///
    /// The queue holds one. Refusing is better than queueing: a second
    /// upload started behind the first would run against a device whose state
    /// the user has forgotten about.
    pub job_pending: bool,
}

impl App {
    pub fn new(start_dir: PathBuf) -> Self {
        Self {
            browse_dir: start_dir.clone(),
            device: DeviceState::NotFound,
            screen: Screen::Menu,
            selected: 0,
            log: Vec::new(),
            status: Some(format!("browsing from {}", start_dir.display())),
            quit_confirm: false,
            quitting: false,
            should_quit: false,
            final_message: None,
            preview_generation: 0,
            job_pending: false,
        }
    }

    pub fn busy(&self) -> bool {
        matches!(self.screen, Screen::Running(_))
    }

    /// Whether an action can be run right now.
    ///
    /// Every action ends at the keyboard -- even the two that open a file
    /// browser first -- so all of them are gated on it. A menu that lets you
    /// start something that cannot work is a menu that lies.
    pub fn actions_enabled(&self) -> bool {
        self.device.is_ready() && !self.busy()
    }

    fn note(&mut self, s: impl Into<String>) {
        let s = s.into();
        self.log.push(s.clone());
        // The log is a scrollback of the last few things; the header shows the
        // newest. Bounded so a chatty YUNZII_DEBUG run cannot grow forever.
        if self.log.len() > 200 {
            self.log.remove(0);
        }
        self.status = Some(s);
    }

    /// Handles a key. Returns a job when one should be started.
    pub fn on_key(&mut self, key: Key) -> Option<Job> {
        // Quitting during a job asks first, because the answer costs an
        // unfinished animation on the panel.
        if self.quit_confirm {
            match key {
                Key::Char('y') | Key::Char('Y') | Key::Enter => {
                    self.quit_confirm = false;
                    match &mut self.screen {
                        Screen::Running(r) => {
                            r.cancel.store(true, Ordering::Relaxed);
                            r.cancelling = true;
                            // NOT should_quit: wait for the worker to stop, so
                            // the partial-write warning it produces can be
                            // shown after the terminal is restored.
                            self.quitting = true;
                            self.status = Some("cancelling before quitting…".into());
                        }
                        // It finished while the prompt was up.
                        _ => self.should_quit = true,
                    }
                }
                _ => self.quit_confirm = false,
            }
            return None;
        }

        // One job at a time. Enter is the only key that starts one.
        if self.job_pending && key == Key::Enter {
            self.note("still working on the last one");
            return None;
        }

        let job = match &mut self.screen {
            Screen::Menu => self.menu_key(key),
            Screen::Browse { .. } => self.browse_key(key),
            Screen::Confirm(_) => self.confirm_key(key),
            Screen::Running(_) => self.running_key(key),
        };
        if job.is_some() {
            self.job_pending = true;
        }
        job
    }

    /// Called by the event loop when the wait for a cancelled worker runs out.
    ///
    /// Two seconds, then leave anyway. A UI that hangs forever because a
    /// thread did not answer is worse than one that says so and goes.
    pub fn give_up_waiting(&mut self) {
        if self.quitting && !self.should_quit {
            self.should_quit = true;
            self.final_message = Some(
                "quit before the upload confirmed it had stopped -- whatever is on the \
                 keyboard may be incomplete."
                    .into(),
            );
        }
    }

    fn menu_key(&mut self, key: Key) -> Option<Job> {
        match key {
            Key::Char('q') => {
                self.should_quit = true;
                None
            }
            Key::Up => {
                self.selected = self.selected.saturating_sub(1);
                None
            }
            Key::Down => {
                self.selected = (self.selected + 1).min(Action::ALL.len() - 1);
                None
            }
            Key::Enter => {
                let action = Action::ALL[self.selected];
                if !self.device.is_ready() {
                    self.note("no keyboard: this action needs one");
                    return None;
                }
                match action {
                    Action::SetTime => Some(Job::SetTime),
                    Action::ShowHome => Some(Job::SwitchPage(Page::Home)),
                    Action::ShowPicture => Some(Job::SwitchPage(Page::Picture)),
                    Action::ShowGif => Some(Job::SwitchPage(Page::Gif)),
                    Action::ClearPicture => Some(Job::ClearPicture),
                    Action::UploadPicture | Action::UploadGif => {
                        self.open_browser(action == Action::UploadGif);
                        None
                    }
                }
            }
            _ => None,
        }
    }

    fn open_browser(&mut self, for_gif: bool) {
        // The remembered directory, not `current_dir()`: the status line says
        // where browsing starts, and the two must not disagree.
        let dir = self.browse_dir.clone();
        let (entries, error) = read_dir(&dir, for_gif);
        self.screen = Screen::Browse {
            for_gif,
            dir,
            entries,
            selected: 0,
            error,
        };
    }

    fn browse_key(&mut self, key: Key) -> Option<Job> {
        let Screen::Browse {
            for_gif,
            dir,
            entries,
            selected,
            error,
        } = &mut self.screen
        else {
            return None;
        };
        let for_gif = *for_gif;

        match key {
            Key::Esc | Key::Char('q') => {
                // The result is no longer wanted, but the worker is still
                // decoding: `job_pending` stays set until it answers, or the
                // menu could hand it a second job while it is busy.
                self.preview_generation += 1;
                self.screen = Screen::Menu;
                None
            }
            Key::Up => {
                *selected = selected.saturating_sub(1);
                None
            }
            Key::Down => {
                *selected = (*selected + 1).min(entries.len().saturating_sub(1));
                None
            }
            Key::Backspace => {
                // At the root this does nothing rather than looping.
                if let Some(parent) = dir.parent().map(|p| p.to_path_buf()) {
                    let (e, err) = read_dir(&parent, for_gif);
                    self.browse_dir = parent.clone();
                    *dir = parent;
                    *entries = e;
                    *error = err;
                    *selected = 0;
                }
                None
            }
            Key::Char('~') => {
                match std::env::var_os("HOME").map(PathBuf::from) {
                    Some(home) => {
                        let (e, err) = read_dir(&home, for_gif);
                        self.browse_dir = home.clone();
                        *dir = home;
                        *entries = e;
                        *error = err;
                        *selected = 0;
                    }
                    None => *error = Some("no home directory".into()),
                }
                None
            }
            Key::Enter => {
                let entry = entries.get(*selected)?.clone();
                if entry.is_dir {
                    let (e, err) = read_dir(&entry.path, for_gif);
                    self.browse_dir = entry.path.clone();
                    *dir = entry.path;
                    *entries = e;
                    *error = err;
                    *selected = 0;
                    None
                } else {
                    self.note(format!("reading {}…", entry.name));
                    self.preview_generation += 1;
                    Some(Job::Preview {
                        path: entry.path,
                        for_gif,
                        generation: self.preview_generation,
                    })
                }
            }
            _ => None,
        }
    }

    fn confirm_key(&mut self, key: Key) -> Option<Job> {
        match key {
            Key::Esc | Key::Char('q') => {
                self.screen = Screen::Menu;
                self.note("discarded");
                None
            }
            Key::Up | Key::Down => {
                let Screen::Confirm(p) = &mut self.screen else {
                    return None;
                };
                let last = p.rows().len() - 1;
                let cur = p.row();
                let next = if key == Key::Up {
                    cur.saturating_sub(1)
                } else {
                    (cur + 1).min(last)
                };
                match p.as_mut() {
                    Pending::Picture { row, .. } | Pending::Gif { row, .. } => *row = next,
                }
                None
            }
            Key::Left | Key::Right => {
                let Screen::Confirm(p) = &mut self.screen else {
                    return None;
                };
                let up = key == Key::Right;
                let row = p.rows()[p.row()];
                let changed = adjust_row(p.as_mut(), row, up);
                if changed {
                    p.reencode_preview();
                }
                None
            }
            Key::Char(' ') => {
                let Screen::Confirm(p) = &mut self.screen else {
                    return None;
                };
                let row = p.rows()[p.row()];
                if toggle_row(p.as_mut(), row) {
                    p.reencode_preview();
                }
                None
            }
            Key::Char('0') => {
                let Screen::Confirm(p) = &mut self.screen else {
                    return None;
                };
                match p.as_mut() {
                    Pending::Picture { adjustments, .. } | Pending::Gif { adjustments, .. } => {
                        *adjustments = Adjustments::NONE;
                    }
                }
                p.reencode_preview();
                self.note("adjustments reset");
                None
            }
            Key::Enter => {
                if !self.device.is_ready() {
                    self.note("no keyboard: cannot upload");
                    return None;
                }
                let Screen::Confirm(mut p) = std::mem::replace(&mut self.screen, Screen::Menu)
                else {
                    return None;
                };
                // Every frame, from the pristine pixels, so what was previewed
                // is what goes out.
                p.reencode_all();
                match *p {
                    Pending::Picture { plan, .. } => Some(Job::UploadPicture(Box::new(plan))),
                    Pending::Gif {
                        mut plan,
                        rate_override,
                        ..
                    } => {
                        if let Some(r) = rate_override {
                            plan.rate = r;
                        }
                        Some(Job::UploadGif(Box::new(plan)))
                    }
                }
            }
            _ => None,
        }
    }

    fn running_key(&mut self, key: Key) -> Option<Job> {
        match key {
            Key::Esc => {
                if let Screen::Running(r) = &mut self.screen {
                    r.cancel.store(true, Ordering::Relaxed);
                    r.cancelling = true;
                    self.status = Some("cancelling…".into());
                }
                None
            }
            Key::Char('q') => {
                self.quit_confirm = true;
                None
            }
            _ => None,
        }
    }

    pub fn on_update(&mut self, update: Update) {
        match update {
            Update::Device(d) => {
                if d != self.device {
                    self.note(format!("device: {}", d.summary()));
                }
                self.device = d;
            }
            Update::Preview { generation, result } => {
                // The worker is free either way -- that is what this says.
                self.job_pending = false;
                // But a reply for a superseded or abandoned request is not
                // applied: it would drag the user into a screen they left.
                if generation != self.preview_generation {
                    return;
                }
                match *result {
                    Ok(pending) => self.screen = Screen::Confirm(Box::new(pending)),
                    Err(e) => {
                        self.note(e);
                        // Stay in the browser so another file can be picked --
                        // and re-read it, since the usual reason a preview
                        // fails is that the file moved or vanished.
                        if let Screen::Browse {
                            for_gif,
                            dir,
                            entries,
                            selected,
                            error,
                        } = &mut self.screen
                        {
                            let (e, err) = read_dir(dir, *for_gif);
                            *entries = e;
                            *error = err;
                            *selected = (*selected).min(entries.len().saturating_sub(1));
                        }
                    }
                }
            }
            Update::Started {
                label,
                total,
                est_secs,
                cancel,
            } => {
                self.screen = Screen::Running(Running {
                    label,
                    done: 0,
                    total,
                    frame: None,
                    started: Instant::now(),
                    est_secs,
                    cancel,
                    cancelling: false,
                });
            }
            Update::Progress { done, total, frame } => {
                if let Screen::Running(r) = &mut self.screen {
                    r.done = done;
                    r.total = total;
                    if frame.is_some() {
                        r.frame = frame;
                    }
                }
            }
            Update::Note(m) => self.note(m),
            Update::Finished(result) => {
                self.job_pending = false;
                self.screen = Screen::Menu;
                let text = match result {
                    Ok(m) => m,
                    Err(e) => e,
                };
                self.note(text.clone());
                // A quit that was waiting for the worker can happen now, and
                // whatever the worker said goes to the normal screen.
                if self.quitting {
                    self.should_quit = true;
                    self.final_message = Some(text);
                }
            }
        }
    }
}

/// Nudges the selected row. Returns whether the image needs re-encoding --
/// the rate does not change a single pixel, so it does not.
fn adjust_row(p: &mut Pending, row: Row, up: bool) -> bool {
    let step = if up { ADJUST_STEP } else { -ADJUST_STEP };
    match row {
        Row::Rate => {
            if let Pending::Gif {
                plan,
                rate_override,
                ..
            } = p
            {
                let current = rate_override.unwrap_or(plan.rate);
                let next = if up {
                    (current + 1).min(crate::protocol::GIF_FPS_MAX)
                } else {
                    current.saturating_sub(1).max(crate::protocol::GIF_FPS_MIN)
                };
                // Back at the file's own rate is not a choice to override it,
                // so the fallback note applies again.
                *rate_override = (next != plan.rate).then_some(next);
            }
            false
        }
        Row::Brightness | Row::Chroma | Row::Saturation => {
            let adj = match p {
                Pending::Picture { adjustments, .. } | Pending::Gif { adjustments, .. } => {
                    adjustments
                }
            };
            let field = match row {
                Row::Brightness => &mut adj.brightness,
                Row::Chroma => &mut adj.chroma,
                _ => &mut adj.saturation,
            };
            // Rounded to the step so repeated presses cannot drift off it.
            let next = ((*field + step) / ADJUST_STEP).round() * ADJUST_STEP;
            let next = next.clamp(-1.0, 1.0);
            let changed = next != *field;
            *field = next;
            changed
        }
        Row::Grayscale | Row::Sharpen | Row::Blur => toggle_row(p, row),
    }
}

/// Flips a boolean row. Returns whether anything changed.
fn toggle_row(p: &mut Pending, row: Row) -> bool {
    let adj = match p {
        Pending::Picture { adjustments, .. } | Pending::Gif { adjustments, .. } => adjustments,
    };
    match row {
        Row::Grayscale => adj.grayscale = !adj.grayscale,
        Row::Sharpen => adj.sharpen = !adj.sharpen,
        Row::Blur => adj.blur = !adj.blur,
        _ => return false,
    }
    true
}

/// Directories, plus the image files the given command accepts.
///
/// Extensions are matched case-insensitively: `PHOTO.JPG` off a camera is a
/// real file people have.
fn read_dir(dir: &std::path::Path, for_gif: bool) -> (Vec<Entry>, Option<String>) {
    let exts: &[&str] = if for_gif {
        &["gif"]
    } else {
        &["png", "jpg", "jpeg"]
    };

    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) => {
            return (
                Vec::new(),
                Some(format!("cannot read {}: {e}", dir.display())),
            );
        }
    };

    let mut entries: Vec<Entry> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let is_dir = path.is_dir();
            let keep = is_dir
                || path
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| exts.contains(&x.to_ascii_lowercase().as_str()))
                    .unwrap_or(false);
            keep.then_some(Entry { name, path, is_dir })
        })
        .collect();

    entries
        .sort_by(|a, b| (b.is_dir, a.name.to_lowercase()).cmp(&(a.is_dir, b.name.to_lowercase())));
    (entries, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan;
    use std::path::Path;

    fn app_ready() -> App {
        let mut a = App::new(PathBuf::from("."));
        a.device = DeviceState::Ready(PathBuf::from("/dev/hidraw5"));
        a
    }

    fn gif_pending() -> Pending {
        let plan = plan::plan_gif_upload(
            Path::new("fixtures/test-anim-2frames.gif"),
            None,
            None,
            &Adjustments::NONE,
        )
        .unwrap();
        Pending::Gif {
            path: PathBuf::from("fixtures/test-anim-2frames.gif"),
            plan,
            rate_override: None,
            adjustments: Adjustments::NONE,
            row: 0,
        }
    }

    // --- The menu ---

    #[test]
    fn nothing_can_be_started_without_a_keyboard() {
        let mut a = App::new(PathBuf::from("."));
        assert!(!a.actions_enabled());
        for i in 0..Action::ALL.len() {
            a.selected = i;
            assert!(
                a.on_key(Key::Enter).is_none(),
                "{:?} must not start without a device",
                Action::ALL[i]
            );
        }
        assert!(a.status.as_deref().unwrap().contains("no keyboard"));
    }

    #[test]
    fn the_selection_stops_at_both_ends() {
        let mut a = app_ready();
        a.on_key(Key::Up);
        assert_eq!(a.selected, 0, "already at the top");
        for _ in 0..20 {
            a.on_key(Key::Down);
        }
        assert_eq!(a.selected, Action::ALL.len() - 1, "and at the bottom");
    }

    #[test]
    fn each_menu_entry_starts_the_job_its_label_promises() {
        let cases = [
            (Action::SetTime, "SetTime"),
            (Action::ShowHome, "SwitchPage(Home)"),
            (Action::ShowPicture, "SwitchPage(Picture)"),
            (Action::ShowGif, "SwitchPage(Gif)"),
            (Action::ClearPicture, "ClearPicture"),
        ];
        for (action, expected) in cases {
            let mut a = app_ready();
            a.selected = Action::ALL.iter().position(|x| *x == action).unwrap();
            let job = a.on_key(Key::Enter).expect("a job");
            assert_eq!(format!("{job:?}"), expected, "for {action:?}");
        }
    }

    #[test]
    fn the_upload_entries_open_a_browser_rather_than_uploading() {
        for (action, for_gif) in [(Action::UploadPicture, false), (Action::UploadGif, true)] {
            let mut a = app_ready();
            a.selected = Action::ALL.iter().position(|x| *x == action).unwrap();
            assert!(a.on_key(Key::Enter).is_none(), "no job yet");
            match &a.screen {
                Screen::Browse { for_gif: g, .. } => assert_eq!(*g, for_gif),
                other => panic!("expected the browser, got {other:?}"),
            }
        }
    }

    // --- Preview and the confirm step ---

    /// Nothing reaches the keyboard on the first Enter. This is the whole
    /// reason preview is a separate job: an earlier design had choosing a file
    /// start the upload, which left no moment at which a preview could exist.
    #[test]
    fn choosing_a_file_previews_it_and_uploads_only_on_a_second_key() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: true,
            dir: PathBuf::from("fixtures"),
            entries: vec![Entry {
                name: "test-anim-2frames.gif".into(),
                path: PathBuf::from("fixtures/test-anim-2frames.gif"),
                is_dir: false,
            }],
            selected: 0,
            error: None,
        };

        let job = a.on_key(Key::Enter).expect("a preview job");
        assert!(
            matches!(job, Job::Preview { for_gif: true, .. }),
            "the first Enter previews, it does not upload: {job:?}"
        );

        a.on_update(Update::Preview {
            generation: a.preview_generation,
            result: Box::new(Ok(gif_pending())),
        });
        assert!(matches!(a.screen, Screen::Confirm(_)));

        let job = a.on_key(Key::Enter).expect("now it uploads");
        assert!(matches!(job, Job::UploadGif(_)), "got {job:?}");
    }

    #[test]
    fn discarding_a_preview_sends_nothing() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        assert!(a.on_key(Key::Esc).is_none());
        assert!(matches!(a.screen, Screen::Menu));
    }

    #[test]
    fn a_preview_that_fails_says_so_and_stays_in_the_browser() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: true,
            dir: PathBuf::from("."),
            entries: vec![],
            selected: 0,
            error: None,
        };
        a.on_update(Update::Preview {
            generation: a.preview_generation,
            result: Box::new(Err("could not use x.png as an animation".into())),
        });
        assert!(
            matches!(a.screen, Screen::Browse { .. }),
            "pick another one"
        );
        assert!(a.status.as_deref().unwrap().contains("as an animation"));
    }

    /// Adjusting the rate is the equivalent of `--fps`, and it is what gets
    /// uploaded.
    #[test]
    fn the_rate_can_be_changed_before_uploading() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));

        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        let Pending::Gif { plan, .. } = p.as_ref() else {
            unreachable!()
        };
        let native = plan.rate;

        a.on_key(Key::Right);
        a.on_key(Key::Right);
        let job = a.on_key(Key::Enter).expect("upload");
        match job {
            Job::UploadGif(plan) => assert_eq!(plan.rate, native + 2),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn the_rate_cannot_leave_the_devices_range() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        for _ in 0..200 {
            a.on_key(Key::Right);
        }
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        let Pending::Gif { rate_override, .. } = p.as_ref() else {
            unreachable!()
        };
        assert_eq!(*rate_override, Some(crate::protocol::GIF_FPS_MAX));

        for _ in 0..200 {
            a.on_key(Key::Left);
        }
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        let Pending::Gif { rate_override, .. } = p.as_ref() else {
            unreachable!()
        };
        assert_eq!(*rate_override, Some(crate::protocol::GIF_FPS_MIN));
    }

    // --- While something is running ---

    fn running(a: &mut App) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        a.on_update(Update::Started {
            label: "Uploading GIF".into(),
            total: 100,
            est_secs: Some(45),
            cancel: Arc::clone(&cancel),
        });
        cancel
    }

    #[test]
    fn escape_asks_the_worker_to_stop() {
        let mut a = app_ready();
        let cancel = running(&mut a);
        assert!(!cancel.load(Ordering::Relaxed));

        a.on_key(Key::Esc);
        assert!(cancel.load(Ordering::Relaxed), "the worker is told to stop");
        assert!(!a.should_quit, "cancelling is not quitting");
        assert!(a.status.as_deref().unwrap().contains("cancelling"));
    }

    /// Quitting mid-upload costs an unfinished animation, so it asks.
    #[test]
    fn quitting_during_an_upload_asks_first() {
        let mut a = app_ready();
        let cancel = running(&mut a);

        a.on_key(Key::Char('q'));
        assert!(a.quit_confirm, "it must ask");
        assert!(!a.should_quit);
        assert!(!cancel.load(Ordering::Relaxed), "and not act yet");

        a.on_key(Key::Char('n'));
        assert!(!a.quit_confirm, "answering no returns to the upload");
        assert!(!a.should_quit);
        assert!(!cancel.load(Ordering::Relaxed));

        a.on_key(Key::Char('q'));
        a.on_key(Key::Char('y'));
        assert!(cancel.load(Ordering::Relaxed), "yes cancels");
        assert!(
            !a.should_quit,
            "but it does NOT leave yet: exiting while a report is in flight is how you \
             get a half-written animation and no message about it"
        );
        assert!(a.quitting, "it is waiting for the worker");

        // The worker stops and says what it left behind; only now do we go,
        // and that message is carried out to the normal screen.
        a.on_update(Update::Finished(Err(
            "cancelled -- the animation on the keyboard may be incomplete".into(),
        )));
        assert!(a.should_quit);
        assert_eq!(
            a.final_message.as_deref(),
            Some("cancelled -- the animation on the keyboard may be incomplete"),
            "printed after the terminal is restored, where it can be read"
        );
    }

    /// A worker that never answers must not hold the interface hostage.
    #[test]
    fn a_worker_that_does_not_answer_is_given_up_on() {
        let mut a = app_ready();
        running(&mut a);
        a.on_key(Key::Char('q'));
        a.on_key(Key::Char('y'));
        assert!(!a.should_quit);

        a.give_up_waiting();
        assert!(a.should_quit, "two seconds is the limit, then we leave");
        assert!(
            a.final_message
                .as_deref()
                .unwrap()
                .contains("may be incomplete"),
            "and still warn: {:?}",
            a.final_message
        );
    }

    /// One job at a time; a second Enter is refused rather than queued.
    #[test]
    fn a_second_job_is_refused_while_one_is_running() {
        let mut a = app_ready();
        a.selected = 0; // Set time
        assert!(a.on_key(Key::Enter).is_some(), "the first one starts");
        assert!(a.job_pending);

        assert!(
            a.on_key(Key::Enter).is_none(),
            "the second must not be queued behind it"
        );
        assert!(a.status.as_deref().unwrap().contains("still working"));

        a.on_update(Update::Finished(Ok("set the clock".into())));
        assert!(!a.job_pending, "and the gate opens again");
        assert!(a.on_key(Key::Enter).is_some());
    }

    #[test]
    fn quitting_from_the_menu_does_not_ask() {
        let mut a = app_ready();
        a.on_key(Key::Char('q'));
        assert!(a.should_quit);
        assert!(!a.quit_confirm);
    }

    #[test]
    fn progress_and_frames_reach_the_bar() {
        let mut a = app_ready();
        running(&mut a);
        a.on_update(Update::Progress {
            done: 25,
            total: 100,
            frame: Some((3, 36)),
        });
        let Screen::Running(r) = &a.screen else {
            panic!("still running")
        };
        assert_eq!(r.percent(), 25);
        assert_eq!(r.frame, Some((3, 36)));

        // A session report carries no frame; the last known one must stay.
        a.on_update(Update::Progress {
            done: 26,
            total: 100,
            frame: None,
        });
        let Screen::Running(r) = &a.screen else {
            unreachable!()
        };
        assert_eq!(
            r.frame,
            Some((3, 36)),
            "the frame number must not blink out"
        );
    }

    #[test]
    fn the_estimate_is_the_planners_until_there_is_enough_to_measure() {
        let mut a = app_ready();
        running(&mut a);
        let Screen::Running(r) = &a.screen else {
            unreachable!()
        };
        assert_eq!(r.remaining_secs(), Some(45), "the planner's estimate");

        a.on_update(Update::Progress {
            done: 50,
            total: 100,
            frame: None,
        });
        let Screen::Running(r) = &a.screen else {
            unreachable!()
        };
        // Measured now: half done in almost no time, so almost no time left.
        assert!(r.remaining_secs().unwrap() < 45);
    }

    #[test]
    fn finishing_returns_to_the_menu_and_says_what_happened() {
        let mut a = app_ready();
        running(&mut a);
        a.on_update(Update::Finished(Ok("Uploading GIF: done".into())));
        assert!(matches!(a.screen, Screen::Menu));
        assert!(a.status.as_deref().unwrap().contains("done"));

        running(&mut a);
        a.on_update(Update::Finished(Err("I/O error: broken pipe".into())));
        assert!(matches!(a.screen, Screen::Menu));
        assert!(a.status.as_deref().unwrap().contains("broken pipe"));
    }

    /// A quit that was waiting for the upload to stop happens once it has.
    /// If the job finishes while the prompt is still up, answering yes just
    /// leaves -- there is nothing left to wait for.
    #[test]
    fn confirming_a_quit_after_the_job_ended_leaves_immediately() {
        let mut a = app_ready();
        running(&mut a);
        a.on_key(Key::Char('q'));
        a.on_update(Update::Finished(Ok("done".into())));
        a.on_key(Key::Char('y'));
        assert!(a.should_quit);
        assert!(matches!(a.screen, Screen::Menu));
    }

    // --- Device state ---

    #[test]
    fn every_device_state_says_what_to_do_about_it() {
        let cases = [
            (DeviceState::NotFound, "rescanning"),
            (
                DeviceState::PermissionDenied(PathBuf::from("/dev/hidraw5")),
                "udev",
            ),
            (
                DeviceState::MultipleMatches(vec![PathBuf::from("/a"), PathBuf::from("/b")]),
                "refusing to guess",
            ),
            (DeviceState::ScanFailed("no /sys".into()), "scan failed"),
        ];
        for (state, expected) in cases {
            assert!(!state.is_ready());
            let s = state.summary();
            assert!(
                s.contains(expected),
                "{state:?} should mention {expected}: {s}"
            );
        }
        assert!(DeviceState::Ready(PathBuf::from("/dev/hidraw5")).is_ready());
    }

    #[test]
    fn losing_the_device_mid_session_is_reported_once() {
        let mut a = app_ready();
        a.on_update(Update::Device(DeviceState::NotFound));
        assert!(!a.device.is_ready());
        let logged = a.log.len();
        // The same state again is not worth repeating.
        a.on_update(Update::Device(DeviceState::NotFound));
        assert_eq!(a.log.len(), logged, "no duplicate lines");
    }

    #[test]
    fn the_log_does_not_grow_without_bound() {
        let mut a = app_ready();
        for i in 0..500 {
            a.on_update(Update::Note(format!("line {i}")));
        }
        assert!(a.log.len() <= 200, "got {}", a.log.len());
        assert!(
            a.log.last().unwrap().contains("499"),
            "and keeps the newest"
        );
    }

    // --- The file browser ---

    #[test]
    fn the_browser_lists_only_what_the_command_accepts() {
        let (entries, err) = read_dir(Path::new("fixtures"), true);
        assert!(err.is_none());
        assert!(!entries.is_empty());
        for e in &entries {
            assert!(
                e.is_dir || e.name.to_lowercase().ends_with(".gif"),
                "a GIF picker should not offer {}",
                e.name
            );
        }

        let (entries, _) = read_dir(Path::new("fixtures"), false);
        for e in &entries {
            assert!(
                e.is_dir
                    || ["png", "jpg", "jpeg"]
                        .iter()
                        .any(|x| e.name.to_lowercase().ends_with(x)),
                "a picture picker should not offer {}",
                e.name
            );
        }
    }

    #[test]
    fn an_unreadable_directory_is_an_inline_error_not_a_crash() {
        let (entries, err) = read_dir(Path::new("/nonexistent-directory-xyz"), false);
        assert!(entries.is_empty());
        assert!(err.unwrap().contains("cannot read"));
    }

    #[test]
    fn backspace_at_the_root_does_nothing() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: false,
            dir: PathBuf::from("/"),
            entries: vec![],
            selected: 0,
            error: None,
        };
        a.on_key(Key::Backspace);
        match &a.screen {
            Screen::Browse { dir, .. } => assert_eq!(dir, Path::new("/")),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn escape_leaves_the_browser() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: false,
            dir: PathBuf::from("."),
            entries: vec![],
            selected: 0,
            error: None,
        };
        a.on_key(Key::Esc);
        assert!(matches!(a.screen, Screen::Menu));
    }

    /// The browser opens where it was left, and the status line agrees.
    #[test]
    fn the_browser_remembers_where_it_was() {
        let mut a = app_ready();
        a.browse_dir = PathBuf::from("fixtures");
        a.selected = Action::ALL
            .iter()
            .position(|x| *x == Action::UploadGif)
            .unwrap();
        a.on_key(Key::Enter);
        match &a.screen {
            Screen::Browse { dir, .. } => assert_eq!(dir, Path::new("fixtures")),
            other => panic!("got {other:?}"),
        }
    }

    /// A preview that fails re-reads the directory, because the usual reason
    /// is that the file moved or was deleted since it was listed.
    #[test]
    fn a_failed_preview_refreshes_the_listing() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: true,
            dir: PathBuf::from("fixtures"),
            entries: vec![Entry {
                name: "ghost.gif".into(),
                path: PathBuf::from("fixtures/ghost.gif"),
                is_dir: false,
            }],
            selected: 0,
            error: None,
        };
        a.on_update(Update::Preview {
            generation: a.preview_generation,
            result: Box::new(Err("no such file".into())),
        });

        match &a.screen {
            Screen::Browse { entries, .. } => {
                assert!(
                    !entries.iter().any(|e| e.name == "ghost.gif"),
                    "the stale entry must be gone after a refresh"
                );
                assert!(!entries.is_empty(), "and the real files are listed");
            }
            other => panic!("got {other:?}"),
        }
    }

    /// A preview that arrives after the user walked away is dropped.
    ///
    /// Decoding a long GIF takes a moment. Pressing Esc during it used to be
    /// ignored: the reply still forced the confirm screen, dragging the user
    /// into something they had already dismissed.
    #[test]
    fn a_preview_that_arrives_too_late_is_ignored() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: true,
            dir: PathBuf::from("fixtures"),
            entries: vec![Entry {
                name: "test-anim-2frames.gif".into(),
                path: PathBuf::from("fixtures/test-anim-2frames.gif"),
                is_dir: false,
            }],
            selected: 0,
            error: None,
        };
        let job = a.on_key(Key::Enter).expect("a preview job");
        let Job::Preview { generation, .. } = job else {
            panic!("expected a preview")
        };

        // The user gives up waiting and leaves.
        a.on_key(Key::Esc);
        assert!(matches!(a.screen, Screen::Menu));

        // The worker answers anyway.
        a.on_update(Update::Preview {
            generation,
            result: Box::new(Ok(gif_pending())),
        });
        assert!(
            matches!(a.screen, Screen::Menu),
            "a dismissed preview must not reappear"
        );
    }

    /// And a reply for the request actually in flight still lands.
    #[test]
    fn the_current_preview_still_arrives() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: true,
            dir: PathBuf::from("fixtures"),
            entries: vec![Entry {
                name: "test-anim-2frames.gif".into(),
                path: PathBuf::from("fixtures/test-anim-2frames.gif"),
                is_dir: false,
            }],
            selected: 0,
            error: None,
        };
        let Some(Job::Preview { generation, .. }) = a.on_key(Key::Enter) else {
            panic!("expected a preview")
        };
        a.on_update(Update::Preview {
            generation,
            result: Box::new(Ok(gif_pending())),
        });
        assert!(matches!(a.screen, Screen::Confirm(_)));
    }

    /// A device that fails mid-job stops looking available immediately.
    ///
    /// The interface half of the contract; `worker::tests` covers the other
    /// half -- that the worker actually sends the update rather than only
    /// clearing its own flag, which was the bug.
    #[test]
    fn a_failed_job_takes_the_green_dot_away() {
        let mut a = app_ready();
        assert!(a.actions_enabled());
        a.on_update(Update::Device(DeviceState::NotFound));
        a.on_update(Update::Finished(Err("I/O error: broken pipe".into())));
        assert!(!a.actions_enabled(), "no action may look available now");
        assert!(!a.device.is_ready());
    }

    /// Discarding a preview does not free the worker: it is still decoding.
    #[test]
    fn leaving_the_browser_does_not_unlock_a_busy_worker() {
        let mut a = app_ready();
        a.screen = Screen::Browse {
            for_gif: true,
            dir: PathBuf::from("fixtures"),
            entries: vec![Entry {
                name: "test-anim-2frames.gif".into(),
                path: PathBuf::from("fixtures/test-anim-2frames.gif"),
                is_dir: false,
            }],
            selected: 0,
            error: None,
        };
        let Some(Job::Preview { generation, .. }) = a.on_key(Key::Enter) else {
            panic!("expected a preview")
        };
        assert!(a.job_pending);

        a.on_key(Key::Esc);
        assert!(
            a.job_pending,
            "the decode is still running -- a second job would queue behind it"
        );
        assert!(a.on_key(Key::Enter).is_none(), "so nothing else may start");

        // The abandoned reply frees it, without reopening the screen.
        a.on_update(Update::Preview {
            generation,
            result: Box::new(Ok(gif_pending())),
        });
        assert!(!a.job_pending, "now the worker is free");
        assert!(matches!(a.screen, Screen::Menu), "but stays dismissed");
    }

    /// Returning to the file's own rate is not an override, so the warning
    /// that applies to it comes back.
    #[test]
    fn arrowing_back_to_the_native_rate_clears_the_override() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        a.on_key(Key::Right);
        a.on_key(Key::Left);

        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        let Pending::Gif { rate_override, .. } = p.as_ref() else {
            unreachable!()
        };
        assert_eq!(
            *rate_override, None,
            "back where it started means no choice was made"
        );
    }

    // --- Milestone 6: the adjustment rows ---

    /// The arrows still change the frame rate, because Rate is the first row.
    ///
    /// This is why the confirm screen became a list instead of growing a
    /// modifier key: the behaviour Milestone 5 shipped keeps working
    /// untouched, and adjustments are rows below it.
    #[test]
    fn the_arrows_still_change_the_rate_by_default() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert_eq!(
            p.rows()[p.row()],
            Row::Rate,
            "Rate is where the cursor starts"
        );

        let Pending::Gif { plan, .. } = p.as_ref() else {
            unreachable!()
        };
        let native = plan.rate;

        a.on_key(Key::Right);
        a.on_key(Key::Right);
        let job = a.on_key(Key::Enter).expect("upload");
        match job {
            Job::UploadGif(plan) => assert_eq!(plan.rate, native + 2),
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn the_row_cursor_stops_at_both_ends() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        a.on_key(Key::Up);
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert_eq!(p.row(), 0);

        for _ in 0..30 {
            a.on_key(Key::Down);
        }
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert_eq!(p.row(), Row::GIF.len() - 1);
    }

    #[test]
    fn a_picture_has_no_rate_row() {
        let plan =
            plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png"), &Adjustments::NONE)
                .unwrap();
        let p = Pending::Picture {
            path: PathBuf::from("p.png"),
            plan,
            adjustments: Adjustments::NONE,
            row: 0,
        };
        assert!(!p.rows().contains(&Row::Rate));
        assert_eq!(p.rows()[0], Row::Brightness);
    }

    #[test]
    fn a_slider_moves_by_the_step_and_clamps() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        a.on_key(Key::Down); // -> Brightness

        a.on_key(Key::Right);
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert!((p.adjustments().brightness - 0.05).abs() < 1e-9);

        for _ in 0..100 {
            a.on_key(Key::Right);
        }
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert_eq!(p.adjustments().brightness, 1.0, "clamps at the top");

        for _ in 0..200 {
            a.on_key(Key::Left);
        }
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert_eq!(p.adjustments().brightness, -1.0, "and at the bottom");
    }

    #[test]
    fn space_toggles_each_boolean_row() {
        for (steps, read) in [
            (
                4usize,
                (|a: &Adjustments| a.grayscale) as fn(&Adjustments) -> bool,
            ),
            (5, |a: &Adjustments| a.sharpen),
            (6, |a: &Adjustments| a.blur),
        ] {
            let mut a = app_ready();
            a.screen = Screen::Confirm(Box::new(gif_pending()));
            for _ in 0..steps {
                a.on_key(Key::Down);
            }
            a.on_key(Key::Char(' '));
            let Screen::Confirm(p) = &a.screen else {
                unreachable!()
            };
            assert!(read(p.adjustments()), "row {steps} did not turn on");

            a.on_key(Key::Char(' '));
            let Screen::Confirm(p) = &a.screen else {
                unreachable!()
            };
            assert!(!read(p.adjustments()), "row {steps} did not turn off again");
        }
    }

    #[test]
    fn zero_resets_every_adjustment_but_not_the_rate() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        a.on_key(Key::Right); // rate +1
        a.on_key(Key::Down);
        a.on_key(Key::Right); // brightness
        a.on_key(Key::Down);
        a.on_key(Key::Down);
        a.on_key(Key::Down);
        a.on_key(Key::Char(' ')); // grayscale

        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert!(!p.adjustments().is_identity());

        a.on_key(Key::Char('0'));
        let Screen::Confirm(p) = &a.screen else {
            unreachable!()
        };
        assert!(p.adjustments().is_identity(), "everything back to nothing");
        let Pending::Gif { rate_override, .. } = p.as_ref() else {
            unreachable!()
        };
        assert_eq!(
            *rate_override,
            Some(11),
            "the rate is a separate decision and survives the reset"
        );
    }

    /// Sliding away and back returns the original bytes exactly.
    ///
    /// Only true because every recompute starts from the pristine panel
    /// pixels. Adjusting on top of the previous result would accumulate
    /// rounding and never come home.
    #[test]
    fn adjusting_and_undoing_returns_the_original_pixels() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        let before = match &a.screen {
            Screen::Confirm(p) => match p.as_ref() {
                Pending::Gif { plan, .. } => plan.frames[0].clone(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };

        a.on_key(Key::Down); // Brightness
        for _ in 0..6 {
            a.on_key(Key::Right);
        }
        let during = match &a.screen {
            Screen::Confirm(p) => match p.as_ref() {
                Pending::Gif { plan, .. } => plan.frames[0].clone(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        assert_ne!(during, before, "the adjustment did something");

        for _ in 0..6 {
            a.on_key(Key::Left);
        }
        let after = match &a.screen {
            Screen::Confirm(p) => match p.as_ref() {
                Pending::Gif { plan, .. } => plan.frames[0].clone(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };
        assert_eq!(after, before, "back to exactly where it started");
    }

    /// What the preview showed is what gets uploaded.
    #[test]
    fn the_previewed_frame_equals_the_uploaded_frame() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        a.on_key(Key::Down);
        a.on_key(Key::Right);
        a.on_key(Key::Down);
        a.on_key(Key::Down);
        a.on_key(Key::Down);
        a.on_key(Key::Char(' ')); // grayscale on

        let previewed = match &a.screen {
            Screen::Confirm(p) => p.frame().to_vec(),
            _ => unreachable!(),
        };

        let job = a.on_key(Key::Enter).expect("upload");
        match job {
            Job::UploadGif(plan) => assert_eq!(
                plan.frames[0], previewed,
                "the upload must send the frame that was on screen"
            ),
            other => panic!("got {other:?}"),
        }
    }

    /// Enter re-encodes every frame, not only the previewed one.
    #[test]
    fn uploading_adjusts_all_the_frames() {
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(gif_pending()));
        let plain = match &a.screen {
            Screen::Confirm(p) => match p.as_ref() {
                Pending::Gif { plan, .. } => plan.frames.clone(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        };

        a.on_key(Key::Down);
        a.on_key(Key::Down);
        a.on_key(Key::Down);
        a.on_key(Key::Down);
        a.on_key(Key::Char(' ')); // grayscale

        let job = a.on_key(Key::Enter).expect("upload");
        match job {
            Job::UploadGif(plan) => {
                for (i, (got, was)) in plan.frames.iter().zip(plain.iter()).enumerate() {
                    assert_ne!(got, was, "frame {i} was left unadjusted");
                }
            }
            other => panic!("got {other:?}"),
        }
    }
}
