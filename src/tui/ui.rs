//! Drawing. No decisions live here -- everything it shows is already in `App`.

use crate::tui::app::{Action, App, Pending, Screen};
use crate::tui::preview;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap};

pub fn draw(f: &mut Frame, app: &App) {
    let running = matches!(app.screen, Screen::Running(_));
    let areas = Layout::vertical([
        Constraint::Length(3),                           // header
        Constraint::Min(6),                              // body
        Constraint::Length(if running { 3 } else { 0 }), // progress
        Constraint::Length(3),                           // status
        Constraint::Length(1),                           // keys
    ])
    .split(f.area());

    header(f, areas[0], app);
    match &app.screen {
        Screen::Menu => menu(f, areas[1], app),
        Screen::Browse { .. } => browse(f, areas[1], app),
        Screen::Confirm(p) => confirm(f, areas[1], p),
        Screen::Running(_) => menu(f, areas[1], app),
    }
    if running {
        progress(f, areas[2], app);
    }
    status(f, areas[3], app);
    keys(f, areas[4], app);
}

fn header(f: &mut Frame, area: Rect, app: &App) {
    let (dot, colour) = if app.device.is_ready() {
        ("●", Color::Green)
    } else {
        ("●", Color::Red)
    };
    let line = Line::from(vec![
        Span::styled(dot, Style::default().fg(colour)),
        Span::raw(" "),
        Span::raw(app.device.summary()),
    ]);
    f.render_widget(
        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Yunzii B75 Pro Max "),
        ),
        area,
    );
}

fn menu(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::horizontal([Constraint::Length(26), Constraint::Min(10)]).split(area);

    let items: Vec<ListItem> = Action::ALL
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let enabled = app.actions_enabled();
            let selected = i == app.selected && !app.busy();
            let mut style = Style::default();
            if !enabled {
                style = style.fg(Color::DarkGray);
            }
            if selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            ListItem::new(Line::from(Span::styled(format!(" {} ", a.label()), style)))
        })
        .collect();

    f.render_widget(
        List::new(items).block(Block::default().borders(Borders::ALL).title(" Actions ")),
        cols[0],
    );
    log_pane(f, cols[1], app);
}

fn log_pane(f: &mut Frame, area: Rect, app: &App) {
    let inner = area.height.saturating_sub(2) as usize;
    let start = app.log.len().saturating_sub(inner);
    let lines: Vec<Line> = app.log[start..]
        .iter()
        .map(|l| Line::from(Span::raw(l.clone())))
        .collect();
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL).title(" Log ")),
        area,
    );
}

fn browse(f: &mut Frame, area: Rect, app: &App) {
    let Screen::Browse {
        dir,
        entries,
        selected,
        error,
        for_gif,
    } = &app.screen
    else {
        return;
    };

    let title = if *for_gif {
        " Pick a GIF "
    } else {
        " Pick a PNG or JPEG "
    };

    if let Some(e) = error {
        f.render_widget(
            Paragraph::new(e.as_str())
                .style(Style::default().fg(Color::Red))
                .wrap(Wrap { trim: true })
                .block(Block::default().borders(Borders::ALL).title(title)),
            area,
        );
        return;
    }

    let items: Vec<ListItem> = if entries.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            " (nothing here to upload) ",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        entries
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let label = if e.is_dir {
                    format!(" {}/ ", e.name)
                } else {
                    format!(" {} ", e.name)
                };
                let mut style = Style::default();
                if e.is_dir {
                    style = style.fg(Color::Cyan);
                }
                if i == *selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                ListItem::new(Line::from(Span::styled(label, style)))
            })
            .collect()
    };

    f.render_widget(
        List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{title}· {} ", dir.display())),
        ),
        area,
    );
}

fn confirm(f: &mut Frame, area: Rect, pending: &Pending) {
    let cols = Layout::horizontal([Constraint::Percentage(60), Constraint::Min(24)]).split(area);
    preview_pane(f, cols[0], pending);

    let (name, mut lines) = match pending {
        Pending::Picture { path, plan, .. } => (
            path.file_name().unwrap_or_default().to_string_lossy(),
            vec![
                format!("160x96, {} bytes", plan.pixels.len()),
                format!("{} reports", plan.total_reports),
            ],
        ),
        Pending::Gif {
            path,
            plan,
            rate_override,
            ..
        } => {
            let rate = rate_override.unwrap_or(plan.rate);
            let mut v = vec![
                format!("{} frames of {}", plan.frames.len(), plan.source_count),
                format!(
                    "{rate} fps{}",
                    if rate_override.is_some() {
                        " (chosen)"
                    } else {
                        ""
                    }
                ),
                format!("~{}s to upload", plan.est_secs),
            ];
            for n in &plan.notes {
                v.push(n.text.clone());
            }
            (path.file_name().unwrap_or_default().to_string_lossy(), v)
        }
    };
    lines.push(String::new());
    lines.push("Enter to upload · Esc to discard".into());
    if matches!(pending, Pending::Gif { .. }) {
        lines.push("← → to change the rate".into());
    }

    f.render_widget(
        Paragraph::new(lines.join("\n"))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {name} ")),
            ),
        cols[1],
    );
}

/// The frame as it will reach the panel, in half-blocks.
fn preview_pane(f: &mut Frame, area: Rect, pending: &Pending) {
    let block = Block::default().borders(Borders::ALL).title(" Preview ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let source = match pending {
        Pending::Picture { preview, .. } | Pending::Gif { preview, .. } => preview,
    };

    let Some((w, h)) = preview::fit(inner.width as usize, inner.height as usize) else {
        f.render_widget(
            Paragraph::new("(pane too small for a preview)")
                .style(Style::default().fg(Color::DarkGray)),
            inner,
        );
        return;
    };

    // Resample the stored preview down to what fits.
    let sw = source.width().max(1);
    let sh = source.height().max(1);
    let lines: Vec<Line> = (0..h)
        .map(|y| {
            let sy = y * sh / h;
            let spans: Vec<Span> = (0..w)
                .map(|x| {
                    let sx = x * sw / w;
                    let (upper, lower) = source.rows[sy.min(sh - 1)][sx.min(sw - 1)];
                    Span::styled(
                        "▀",
                        Style::default()
                            .fg(Color::Rgb(upper.0, upper.1, upper.2))
                            .bg(Color::Rgb(lower.0, lower.1, lower.2)),
                    )
                })
                .collect();
            Line::from(spans)
        })
        .collect();

    // Centre it, so a small preview does not cling to the corner.
    let pad_x = inner.width.saturating_sub(w as u16) / 2;
    let pad_y = inner.height.saturating_sub(h as u16) / 2;
    let target = Rect {
        x: inner.x + pad_x,
        y: inner.y + pad_y,
        width: (w as u16).min(inner.width),
        height: (h as u16).min(inner.height),
    };
    f.render_widget(Paragraph::new(lines), target);
}

fn progress(f: &mut Frame, area: Rect, app: &App) {
    let Screen::Running(r) = &app.screen else {
        return;
    };
    let frame = r
        .frame
        .map(|(i, of)| format!("frame {}/{of}  ", i + 1))
        .unwrap_or_default();
    let left = r
        .remaining_secs()
        .map(|s| format!("  ~{s}s left"))
        .unwrap_or_default();
    let label = if r.cancelling {
        format!("{frame}cancelling…")
    } else {
        format!("{frame}{}%{left}", r.percent())
    };

    f.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} ", r.label)),
            )
            .gauge_style(Style::default().fg(if r.cancelling {
                Color::Yellow
            } else {
                Color::Green
            }))
            .percent(r.percent())
            .label(label),
        area,
    );
}

fn status(f: &mut Frame, area: Rect, app: &App) {
    let text = if app.quit_confirm {
        "an upload is in progress — cancel it and quit? (y/n)".to_string()
    } else {
        app.status.clone().unwrap_or_default()
    };
    let style = if app.quit_confirm {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    f.render_widget(
        Paragraph::new(text)
            .style(style)
            .wrap(Wrap { trim: true })
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn keys(f: &mut Frame, area: Rect, app: &App) {
    let hint = match &app.screen {
        Screen::Menu => "q quit · ↑↓ move · ⏎ run",
        Screen::Browse { .. } => "esc back · ↑↓ move · ⏎ open · ⌫ parent · ~ home",
        Screen::Confirm(_) => "esc discard · ⏎ upload · ← → rate",
        Screen::Running(_) => "esc cancel · q quit",
    };
    f.render_widget(
        Paragraph::new(Span::styled(hint, Style::default().fg(Color::DarkGray))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan;
    use crate::tui::app::{DeviceState, Key, Update};
    use crate::tui::preview;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    /// Renders into a fixed-size buffer and returns it as text.
    fn render_at(app: &App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| draw(f, app)).unwrap();
        let buf = t.backend().buffer().clone();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn app_ready() -> App {
        let mut a = App::new(PathBuf::from("."));
        a.device = DeviceState::Ready(PathBuf::from("/dev/hidraw5"));
        a
    }

    #[test]
    fn the_menu_names_the_device_and_every_action() {
        let out = render_at(&app_ready(), 100, 30);
        assert!(out.contains("/dev/hidraw5"), "{out}");
        for a in Action::ALL {
            let label = a.label().trim_end_matches('…');
            assert!(out.contains(label), "missing {label} in:\n{out}");
        }
    }

    /// The state that greets anyone without the udev rule installed.
    #[test]
    fn a_permission_problem_is_shown_with_its_fix() {
        let mut a = App::new(PathBuf::from("."));
        a.device = DeviceState::PermissionDenied(PathBuf::from("/dev/hidraw5"));
        let out = render_at(&a, 100, 30);
        assert!(out.contains("permission denied"), "{out}");
        assert!(out.contains("udev"), "and how to fix it:\n{out}");
    }

    #[test]
    fn the_progress_bar_shows_the_frame_and_the_percentage() {
        let mut a = app_ready();
        a.on_update(Update::Started {
            label: "Uploading GIF".into(),
            total: 100,
            est_secs: Some(45),
            cancel: Arc::new(AtomicBool::new(false)),
        });
        a.on_update(Update::Progress {
            done: 33,
            total: 100,
            frame: Some((11, 36)),
        });

        let out = render_at(&a, 100, 30);
        assert!(out.contains("Uploading GIF"), "{out}");
        assert!(out.contains("frame 12/36"), "one-based for humans:\n{out}");
        assert!(out.contains("33%"), "{out}");
    }

    #[test]
    fn cancelling_says_so_instead_of_showing_a_percentage() {
        let mut a = app_ready();
        a.on_update(Update::Started {
            label: "Uploading GIF".into(),
            total: 100,
            est_secs: None,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        a.on_update(Update::Progress {
            done: 10,
            total: 100,
            frame: None,
        });
        a.on_key(Key::Esc);
        let out = render_at(&a, 100, 30);
        assert!(out.contains("cancelling"), "{out}");
    }

    #[test]
    fn the_quit_prompt_is_visible_and_says_what_it_costs() {
        let mut a = app_ready();
        a.on_update(Update::Started {
            label: "Uploading GIF".into(),
            total: 100,
            est_secs: None,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        a.on_key(Key::Char('q'));
        let out = render_at(&a, 100, 30);
        assert!(out.contains("cancel it and quit"), "{out}");
    }

    #[test]
    fn the_preview_pane_draws_half_blocks() {
        let plan = plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(Pending::Picture {
            path: PathBuf::from("fixtures/test-quadrants.png"),
            preview: preview::render(&plan.pixels, 80, 24),
            plan,
        }));

        let out = render_at(&a, 100, 30);
        assert!(
            out.contains('▀'),
            "the preview should be half-blocks:\n{out}"
        );
        assert!(out.contains("test-quadrants.png"), "{out}");
        assert!(out.contains("Enter to upload"), "{out}");
    }

    /// Small terminals must degrade, not crash. A layout panic here would take
    /// the whole program down while the user's screen is in raw mode.
    #[test]
    fn every_screen_survives_a_tiny_terminal() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), None, None).unwrap();

        let mut screens: Vec<App> = Vec::new();
        screens.push(app_ready());
        {
            let mut a = app_ready();
            a.screen = Screen::Browse {
                for_gif: true,
                dir: PathBuf::from("fixtures"),
                entries: vec![],
                selected: 0,
                error: Some("cannot read /root: permission denied".into()),
            };
            screens.push(a);
        }
        {
            let mut a = app_ready();
            a.screen = Screen::Confirm(Box::new(Pending::Gif {
                path: PathBuf::from("x.gif"),
                preview: preview::render(&plan.frames[0], 40, 12),
                plan,
                rate_override: Some(24),
            }));
            screens.push(a);
        }
        {
            let mut a = app_ready();
            a.on_update(Update::Started {
                label: "Uploading GIF".into(),
                total: 100,
                est_secs: Some(45),
                cancel: Arc::new(AtomicBool::new(false)),
            });
            screens.push(a);
        }

        for (i, a) in screens.iter().enumerate() {
            for (w, h) in [(40, 12), (20, 5), (8, 3), (1, 1), (200, 60)] {
                let _ = render_at(a, w, h); // must not panic
            }
            let _ = i;
        }
    }

    /// A narrow terminal drops the picture and keeps the facts.
    ///
    /// Two mechanisms combine, and both are wanted: the layout gives the
    /// preview pane nothing when there is not room for both columns, and
    /// `preview::fit` refuses a pane too short to be honest. Either way the
    /// metadata survives, which is the part you cannot guess by looking.
    #[test]
    fn a_narrow_terminal_keeps_the_facts_and_drops_the_picture() {
        let plan = plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        let mut a = app_ready();
        a.screen = Screen::Confirm(Box::new(Pending::Picture {
            path: PathBuf::from("p.png"),
            preview: preview::render(&plan.pixels, 40, 12),
            plan,
        }));

        let narrow = render_at(&a, 24, 20);
        assert!(
            !narrow.contains('▀'),
            "no half-blocks in a pane this size:\n{narrow}"
        );
        assert!(
            narrow.contains("30720 bytes"),
            "but the metadata is still there:\n{narrow}"
        );

        // Given room, it draws. The threshold is not simply "never".
        let wide = render_at(&a, 90, 24);
        assert!(wide.contains('▀'), "{wide}");

        // Short does not remove it: the body keeps a six-row minimum, which
        // leaves the pane exactly the four rows `preview::fit` needs. Height
        // alone never starves the preview -- width does, by collapsing the
        // column entirely. The "pane too small" fallback in `preview_pane` is
        // therefore defensive rather than something this layout reaches, and
        // it is kept because a future layout change should degrade rather
        // than draw a smear.
        assert!(render_at(&a, 90, 13).contains('▀'), "four rows is enough");
        assert!(render_at(&a, 90, 12).contains('▀'), "and so is a short one");
    }
}
