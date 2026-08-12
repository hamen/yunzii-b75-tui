//! What the interface *is*, separated from how it is drawn.
//!
//! Everything here is a plain struct and two functions over it: `on_key` and
//! `on_update`. No terminal, no device, no threads. That is what makes the
//! interesting behaviour -- disabled actions, the confirm step, cancelling,
//! quitting mid-upload -- ordinary unit tests rather than something only a
//! person with a keyboard plugged in can check.

use crate::plan::{GifPlan, PicturePlan};
use crate::protocol::Page;
use crate::tui::preview::Preview;
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
        preview: Preview,
    },
    Gif {
        path: PathBuf,
        plan: GifPlan,
        preview: Preview,
        /// `None` means "as the file asks"; `Some` is the equivalent of
        /// passing `--fps`, and suppresses the fallback note the same way.
        rate_override: Option<u8>,
    },
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
    Preview { path: PathBuf, for_gif: bool },
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
    Preview(Box<Result<Pending, String>>),
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
    pub device: DeviceState,
    pub screen: Screen,
    pub selected: usize,
    pub log: Vec<String>,
    pub status: Option<String>,
    /// Set when the user asked to quit during a job and has not answered yet.
    pub quit_confirm: bool,
    pub should_quit: bool,
}

impl App {
    pub fn new(start_dir: PathBuf) -> Self {
        Self {
            device: DeviceState::NotFound,
            screen: Screen::Menu,
            selected: 0,
            log: Vec::new(),
            status: Some(format!("browsing from {}", start_dir.display())),
            quit_confirm: false,
            should_quit: false,
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
                    if let Screen::Running(r) = &mut self.screen {
                        r.cancel.store(true, Ordering::Relaxed);
                        r.cancelling = true;
                    }
                    self.should_quit = true;
                }
                _ => self.quit_confirm = false,
            }
            return None;
        }

        match &mut self.screen {
            Screen::Menu => self.menu_key(key),
            Screen::Browse { .. } => self.browse_key(key),
            Screen::Confirm(_) => self.confirm_key(key),
            Screen::Running(_) => self.running_key(key),
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
        let dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"));
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
                    *dir = entry.path;
                    *entries = e;
                    *error = err;
                    *selected = 0;
                    None
                } else {
                    self.note(format!("reading {}…", entry.name));
                    Some(Job::Preview {
                        path: entry.path,
                        for_gif,
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
            Key::Left | Key::Right => {
                let Screen::Confirm(p) = &mut self.screen else {
                    return None;
                };
                if let Pending::Gif {
                    plan,
                    rate_override,
                    ..
                } = p.as_mut()
                {
                    let current = rate_override.unwrap_or(plan.rate);
                    let next = if key == Key::Left {
                        current.saturating_sub(1).max(crate::protocol::GIF_FPS_MIN)
                    } else {
                        (current + 1).min(crate::protocol::GIF_FPS_MAX)
                    };
                    *rate_override = Some(next);
                }
                None
            }
            Key::Enter => {
                if !self.device.is_ready() {
                    self.note("no keyboard: cannot upload");
                    return None;
                }
                let Screen::Confirm(p) = std::mem::replace(&mut self.screen, Screen::Menu) else {
                    return None;
                };
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
            Update::Preview(result) => match *result {
                Ok(pending) => self.screen = Screen::Confirm(Box::new(pending)),
                Err(e) => {
                    self.note(e);
                    // Stay in the browser so another file can be picked.
                }
            },
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
                self.screen = Screen::Menu;
                match result {
                    Ok(m) => self.note(m),
                    Err(e) => self.note(e),
                }
                // A quit that was waiting on cancellation can now happen.
                if self.quit_confirm {
                    self.should_quit = true;
                }
            }
        }
    }
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
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), None, None).unwrap();
        Pending::Gif {
            path: PathBuf::from("fixtures/test-anim-2frames.gif"),
            preview: crate::tui::preview::render(&plan.frames[0], 8, 4),
            plan,
            rate_override: None,
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

        a.on_update(Update::Preview(Box::new(Ok(gif_pending()))));
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
        a.on_update(Update::Preview(Box::new(Err(
            "could not use x.png as an animation".into(),
        ))));
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
        assert!(a.should_quit);
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
    #[test]
    fn a_pending_quit_completes_when_the_job_ends() {
        let mut a = app_ready();
        running(&mut a);
        a.on_key(Key::Char('q'));
        a.on_key(Key::Char('y'));
        assert!(a.should_quit);

        // And if the worker reports afterwards, that is still a clean exit.
        a.on_update(Update::Finished(Err("cancelled".into())));
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
}
