//! Drawing. Pure functions of `App` state.

use super::{App, Mode, Tab};
use crate::clean::describe;
use crate::config::gb;
use crate::i18n::{Msg, t, tf};
use crate::model::{Action, Candidate, Safety};
use crate::sys::{self, fmt_bytes, fmt_delta, fmt_mem, fmt_time, tilde};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Cell, Clear, Paragraph, Row, Sparkline, Table, Wrap};
use std::path::Path;

const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn panel<'a>(app: &App, title: impl Into<Line<'a>>) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(app.theme.fg(app.theme.border))
        .title(title)
}

fn title_span(app: &App, s: impl std::fmt::Display) -> Span<'static> {
    Span::styled(format!(" {s} "), app.theme.bold(app.theme.accent))
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(8),
        Constraint::Length(1),
    ])
    .areas(f.area());
    draw_header(f, app, header);
    match app.tab {
        Tab::Overview => draw_overview(f, app, body),
        Tab::Folders => draw_folders(f, app, body),
    }
    draw_footer(f, app, footer);
    match &app.mode {
        Mode::Details => draw_details(f, app),
        Mode::Help => draw_help(f, app),
        Mode::Confirm(_) => draw_confirm(f, app),
        Mode::Message(lines) => draw_message(f, app, lines.clone()),
        Mode::Working(..) | Mode::Normal => {}
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let tabs = Line::from(vec![
        tab_span(app, Tab::Overview, &t("tui.tab_overview")),
        Span::styled("│", th.fg(th.border)),
        tab_span(app, Tab::Folders, &t("tui.tab_folders")),
    ]);
    let when = app
        .report
        .as_ref()
        .map(|r| tf("tui.scanned_at", &[("time", &fmt_time(r.time))]))
        .unwrap_or_default();
    let block = panel(
        app,
        Line::from(vec![
            Span::styled(
                " roomy ",
                th.bold(th.accent).add_modifier(Modifier::REVERSED),
            ),
            Span::styled(format!(" {} ", t("tui.title")), th.bold(th.text)),
        ]),
    )
    .title(tabs.alignment(Alignment::Center))
    .title(Line::styled(format!(" {when} "), th.fg(th.dim)).alignment(Alignment::Right));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [disk_row, mem_row] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let used = app.disk.total.saturating_sub(app.disk.avail);
    let ratio = if app.disk.total > 0 {
        used as f64 / app.disk.total as f64
    } else {
        0.0
    };
    let color = if app.disk.avail < gb(app.cfg.thresholds.disk_critical_gb) {
        th.danger
    } else if app.disk.avail < gb(app.cfg.thresholds.disk_warn_gb) {
        th.review
    } else {
        th.safe
    };
    let disk_label = tf(
        "status.disk_line",
        &[
            ("free", &fmt_bytes(app.disk.avail)),
            ("total", &fmt_bytes(app.disk.total)),
            ("pct", &((ratio * 100.0).round() as u64)),
        ],
    );
    let disk = (ratio, color);

    let mem_used = app
        .mem
        .free_pct
        .map_or(0.0, |p| (100 - p).clamp(0, 100) as f64 / 100.0);
    let mem_color = match app.mem.pressure {
        4 => th.danger,
        2 => th.review,
        _ => th.accent,
    };
    let mem_label = tf(
        "status.memory_line",
        &[
            ("total", &fmt_mem(app.mem.total)),
            (
                "free",
                &app.mem.free_pct.map_or("?".into(), |p| format!("{p}%")),
            ),
            ("pressure", &sys::pressure_label_i18n(app.mem.pressure)),
            ("swap", &fmt_mem(app.mem.swap_used)),
        ],
    );

    // The bars shrink (to 20 columns) before the labels are cut, so the purgeable space fits
    // on common terminal widths; it is left out when it does not.
    let width = |s: &str| unicode_width::UnicodeWidthStr::width(s);
    let max_bar = inner.width as usize * 45 / 100;
    let room = |label: &str| {
        (inner.width as usize).saturating_sub(10 + width(label).max(width(&mem_label)))
    };
    let mut disk_label = disk_label;
    if let Some(p) = sys::purgeable_shown(app.disk.avail, app.disk.avail_incl_purgeable) {
        let full = format!(
            "{disk_label} · {}",
            tf("disk.incl_purgeable", &[("avail", &fmt_bytes(p))])
        );
        if room(&full) >= 20.min(max_bar) {
            disk_label = full;
        }
    }
    let bar_w = room(&disk_label).max(20).min(max_bar) as u16;
    gauge_row(f, app, disk_row, &t("tui.disk"), disk, bar_w, disk_label);
    gauge_row(
        f,
        app,
        mem_row,
        &t("tui.memory"),
        (mem_used, mem_color),
        bar_w,
        mem_label,
    );
}

fn tab_span(app: &App, tab: Tab, label: &str) -> Span<'static> {
    if app.tab == tab {
        Span::styled(
            format!(" {label} "),
            app.theme
                .bold(app.theme.accent)
                .add_modifier(Modifier::UNDERLINED),
        )
    } else {
        Span::styled(format!(" {label} "), app.theme.fg(app.theme.dim))
    }
}

/// A smooth horizontal bar using eighth blocks, e.g. `██████▍░░░`.
pub fn bar(ratio: f64, width: usize, fill: Style, empty: Style) -> Vec<Span<'static>> {
    const PARTS: [&str; 8] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let eighths = (ratio.clamp(0.0, 1.0) * (width * 8) as f64).round() as usize;
    let full = eighths / 8;
    let part = PARTS[eighths % 8];
    let used = full + usize::from(!part.is_empty());
    vec![
        Span::styled("█".repeat(full), fill),
        Span::styled(part, fill),
        Span::styled("░".repeat(width.saturating_sub(used)), empty),
    ]
}

fn gauge_row(
    f: &mut Frame,
    app: &App,
    area: Rect,
    name: &str,
    (ratio, color): (f64, ratatui::style::Color),
    bar_w: u16,
    label: String,
) {
    let [name_area, bar_area, label_area] = Layout::horizontal([
        Constraint::Length(8),
        Constraint::Length(bar_w),
        Constraint::Fill(1),
    ])
    .spacing(1)
    .areas(area);
    f.render_widget(
        Span::styled(name.to_string(), app.theme.bold(app.theme.text)),
        name_area,
    );
    let spans = bar(
        ratio,
        bar_area.width as usize,
        Style::new().fg(color),
        app.theme.fg(app.theme.gauge_bg),
    );
    f.render_widget(Line::from(spans), bar_area);
    f.render_widget(
        Span::styled(label, app.theme.fg(app.theme.text)),
        label_area,
    );
}

fn draw_overview(f: &mut Frame, app: &mut App, area: Rect) {
    let [left, right] =
        Layout::horizontal([Constraint::Percentage(64), Constraint::Percentage(36)]).areas(area);
    draw_candidates(f, app, left);
    let [grew, trend, memory] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(6),
        Constraint::Length(8),
    ])
    .areas(right);
    draw_growth(f, app, grew);
    draw_trend(f, app, trend);
    draw_memory(f, app, memory);
}

fn safety_dot(app: &App, c: &Candidate) -> Span<'static> {
    match c.safety {
        Safety::Safe => Span::styled("●", app.theme.fg(app.theme.safe)),
        Safety::Review => Span::styled("○", app.theme.fg(app.theme.review)),
    }
}

/// Short warning shown next to a candidate, e.g. when a worktree holds files outside Git.
fn tag(c: &Candidate) -> Option<String> {
    let has = |key: &str| {
        c.notes
            .iter()
            .any(|n| matches!(n, Msg::Key { key: k, .. } if k.starts_with(key)))
    };
    if has("wt.unique_files") {
        Some(t("tui.tag_untracked"))
    } else if matches!(c.action, Action::Manual { .. }) {
        Some(t("tui.tag_manual"))
    } else {
        None
    }
}

fn draw_candidates(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let title = match &app.report {
        Some(r) => Line::from(vec![
            title_span(app, t("report.candidates")),
            Span::styled(
                format!("{} {} ", t("safety.safe"), fmt_bytes(r.total(Safety::Safe))),
                th.fg(th.safe),
            ),
            Span::styled(
                format!(
                    "· {} {} ",
                    t("safety.review"),
                    fmt_bytes(r.total(Safety::Review))
                ),
                th.fg(th.review),
            ),
        ]),
        None => Line::from(title_span(app, t("report.candidates"))),
    };
    let marked_bytes: u64 = app.report.as_ref().map_or(0, |r| {
        app.marked
            .iter()
            .filter_map(|i| r.candidates.get(*i))
            .map(Candidate::frees)
            .sum()
    });
    let mut block = panel(app, title);
    if !app.marked.is_empty() {
        block = block.title_bottom(
            Line::styled(
                format!(
                    " {} ",
                    tf(
                        "tui.marked",
                        &[
                            ("n", &app.marked.len()),
                            ("bytes", &fmt_bytes(marked_bytes))
                        ]
                    )
                ),
                th.bold(th.accent),
            )
            .alignment(Alignment::Right),
        );
    }

    if let Mode::Working(label, since) = &app.mode
        && (app.report.is_none() || label == &t("tui.scanning"))
    {
        let frame = SPINNER[(since.elapsed().as_millis() / 100) as usize % SPINNER.len()];
        let text = Text::from(vec![
            Line::raw(""),
            Line::styled(
                format!("{frame} {label}  {}s", since.elapsed().as_secs()),
                th.bold(th.accent),
            ),
            Line::styled(t("tui.scanning_hint"), th.fg(th.dim)),
        ])
        .alignment(Alignment::Center);
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }
    let Some(report) = &app.report else {
        f.render_widget(Paragraph::new(t("clean.no_report")).block(block), area);
        return;
    };
    if report.candidates.is_empty() {
        let text = Text::from(vec![
            Line::raw(""),
            Line::styled(format!("✨ {}", t("report.none")), th.fg(th.safe)),
        ])
        .alignment(Alignment::Center);
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }
    let rows: Vec<Row> = report
        .candidates
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mark = if app.marked.contains(&i) {
                Span::styled("◉", th.bold(th.accent))
            } else {
                Span::raw(" ")
            };
            let mut title = vec![Span::styled(c.title.render(), th.fg(th.text))];
            if let Some(b) = c.frees_less() {
                title.push(Span::styled(
                    format!("  {}", tf("size.frees_short", &[("bytes", &fmt_bytes(b))])),
                    th.fg(th.review),
                ));
            }
            if let Some(tg) = tag(c) {
                title.push(Span::styled(format!("  {tg}"), th.fg(th.review)));
            }
            Row::new(vec![
                Cell::from(mark),
                Cell::from(Line::from(fmt_bytes(c.bytes)).alignment(Alignment::Right)),
                Cell::from(safety_dot(app, c)),
                Cell::from(Line::from(title)),
                Cell::from(Span::styled(c.category.render(), th.fg(th.dim))),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(9),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(12),
        ],
    )
    .block(block)
    .column_spacing(1)
    .row_highlight_style(th.selected())
    .highlight_symbol("▸");
    f.render_stateful_widget(table, area, &mut app.table);
}

fn draw_growth(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let since = app
        .report
        .as_ref()
        .and_then(|r| r.growth_since)
        .map(fmt_time)
        .unwrap_or_default();
    let block = panel(app, title_span(app, t("tui.grew")))
        .title_bottom(Line::styled(format!(" {since} "), th.fg(th.dim)));
    let lines: Vec<Line> = match &app.report {
        Some(r) if !r.growth.is_empty() => r
            .growth
            .iter()
            .take(area.height.saturating_sub(2) as usize)
            .map(|g| {
                Line::from(vec![
                    Span::styled(format!("{:>9} ", fmt_delta(g.delta)), th.bold(th.review)),
                    Span::styled(
                        short_path(&g.path, area.width.saturating_sub(14) as usize),
                        th.fg(th.text),
                    ),
                ])
            })
            .collect(),
        Some(r) if r.growth_since.is_some() => {
            vec![Line::styled(t("tui.no_growth"), th.fg(th.dim))]
        }
        _ => vec![Line::styled(
            t("tui.growth_after_second_scan"),
            th.fg(th.dim),
        )],
    };
    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }).block(block),
        area,
    );
}

/// Keeps the end of a long path, which is the informative part.
fn short_path(p: &str, max: usize) -> String {
    let s = tilde(Path::new(p));
    let n = s.chars().count();
    if n <= max || max < 4 {
        s
    } else {
        format!("…{}", s.chars().skip(n - (max - 1)).collect::<String>())
    }
}

fn draw_trend(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let series = &app.free_series;
    let (lo, hi) = (
        series.iter().copied().min().unwrap_or(0),
        series.iter().copied().max().unwrap_or(0),
    );
    let block = panel(app, title_span(app, t("tui.free_trend"))).title_bottom(
        Line::styled(
            format!(" {} – {} ", fmt_bytes(lo), fmt_bytes(hi)),
            th.fg(th.dim),
        )
        .alignment(Alignment::Right),
    );
    if series.len() < 2 {
        f.render_widget(
            Paragraph::new(Line::styled(t("tui.no_history"), th.fg(th.dim)))
                .wrap(Wrap { trim: true })
                .block(block),
            area,
        );
        return;
    }
    // Scale relative to the lowest value so small changes stay visible.
    let floor = lo.saturating_sub((hi - lo) / 4 + 1);
    let data: Vec<u64> = series.iter().map(|v| (v - floor) / 1_000_000).collect();
    f.render_widget(
        Sparkline::default()
            .block(block)
            .data(&data)
            .style(th.fg(th.accent)),
        area,
    );
}

fn draw_memory(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let block = panel(app, title_span(app, t("tui.memory_top")));
    let inner_w = area.width.saturating_sub(2) as usize;
    let max = app.apps.first().map_or(1, |a| a.rss).max(1);
    let lines: Vec<Line> = app
        .apps
        .iter()
        .map(|a| {
            let bar_w = inner_w.saturating_sub(22).min(12);
            let filled = ((a.rss as f64 / max as f64) * bar_w as f64).round() as usize;
            let name: String = a
                .name
                .chars()
                .take(inner_w.saturating_sub(bar_w + 10))
                .collect();
            Line::from(vec![
                Span::styled(format!("{:>7} ", fmt_mem(a.rss)), th.fg(th.text)),
                Span::styled("▮".repeat(filled), th.fg(th.accent)),
                Span::styled("·".repeat(bar_w - filled), th.fg(th.border)),
                Span::raw(" "),
                Span::styled(name, th.fg(th.dim)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(block), area);
}

pub struct FolderRow {
    pub path: String,
    pub bytes: u64,
    pub delta: Option<i64>,
    pub has_children: bool,
}

pub fn folder_rows(app: &App) -> Vec<FolderRow> {
    let Some(snap) = &app.snap else { return vec![] };
    let key = app.folder.to_string_lossy().into_owned();
    let mut rows: Vec<FolderRow> = snap
        .entries
        .iter()
        .filter(|(p, _)| {
            Path::new(p.as_str())
                .parent()
                .is_some_and(|par| par.to_string_lossy() == key)
        })
        .map(|(p, b)| FolderRow {
            path: p.clone(),
            bytes: *b,
            delta: app
                .prev_snap
                .as_ref()
                .and_then(|pv| pv.entries.get(p))
                .map(|old| *b as i64 - *old as i64),
            has_children: snap.entries.keys().any(|k| {
                k.len() > p.len() && k.starts_with(p.as_str()) && k.as_bytes()[p.len()] == b'/'
            }),
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.bytes));
    rows
}

fn draw_folders(f: &mut Frame, app: &mut App, area: Rect) {
    let th = app.theme;
    let total = app
        .snap
        .as_ref()
        .and_then(|s| s.entries.get(&*app.folder.to_string_lossy()).copied())
        .unwrap_or(0);
    let block = panel(
        app,
        Line::from(vec![
            title_span(app, tilde(&app.folder)),
            Span::styled(format!("{} ", fmt_bytes(total)), th.fg(th.dim)),
        ]),
    );
    let rows = folder_rows(app);
    if rows.is_empty() {
        f.render_widget(Paragraph::new(t("top.empty")).block(block), area);
        return;
    }
    let max = rows.first().map_or(1, |r| r.bytes).max(1);
    let bar_w = 24usize;
    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            let name = Path::new(&r.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let filled = ((r.bytes as f64 / max as f64) * bar_w as f64).round() as usize;
            let delta = match r.delta {
                Some(d) if d.unsigned_abs() >= 100_000_000 => {
                    Span::styled(fmt_delta(d), th.fg(if d > 0 { th.review } else { th.safe }))
                }
                None if app.prev_snap.is_some() => Span::styled(t("top.new"), th.fg(th.dim)),
                _ => Span::raw(""),
            };
            let arrow = if r.has_children { "›" } else { " " };
            Row::new(vec![
                Cell::from(Line::from(fmt_bytes(r.bytes)).alignment(Alignment::Right)),
                Cell::from(Line::from(vec![
                    Span::styled("█".repeat(filled), th.fg(th.accent)),
                    Span::styled("░".repeat(bar_w - filled), th.fg(th.border)),
                ])),
                Cell::from(Line::from(vec![
                    Span::styled(name, th.fg(th.text)),
                    Span::styled(format!(" {arrow}"), th.fg(th.dim)),
                ])),
                Cell::from(Line::from(delta).alignment(Alignment::Right)),
            ])
        })
        .collect();
    let table = Table::new(
        table_rows,
        [
            Constraint::Length(9),
            Constraint::Length(bar_w as u16),
            Constraint::Fill(1),
            Constraint::Length(10),
        ],
    )
    .block(block)
    .column_spacing(2)
    .row_highlight_style(th.selected())
    .highlight_symbol("▸");
    f.render_stateful_widget(table, area, &mut app.folder_table);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let th = &app.theme;
    let keys: Vec<(&str, String)> = match (&app.mode, app.tab) {
        (Mode::Working(..), _) => vec![("q", t("tui.key_quit"))],
        (_, Tab::Folders) => vec![
            ("↑↓", t("tui.key_move")),
            ("⏎/→", t("tui.key_open")),
            ("←", t("tui.key_up")),
            ("tab", t("tui.key_tab")),
            ("r", t("tui.key_rescan")),
            ("q", t("tui.key_quit")),
        ],
        _ => vec![
            ("↑↓", t("tui.key_move")),
            ("␣", t("tui.key_mark")),
            ("a", t("tui.key_all_safe")),
            ("⏎", t("tui.key_details")),
            ("c", t("tui.key_clean")),
            ("r", t("tui.key_rescan")),
            ("tab", t("tui.key_tab")),
            ("?", t("tui.key_help")),
            ("q", t("tui.key_quit")),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in keys {
        spans.push(Span::styled(
            format!(" {k} "),
            th.bold(th.accent).add_modifier(Modifier::REVERSED),
        ));
        spans.push(Span::styled(format!(" {label}  "), th.fg(th.dim)));
    }
    if let Mode::Working(label, since) = &app.mode {
        let frame = SPINNER[(since.elapsed().as_millis() / 100) as usize % SPINNER.len()];
        spans.push(Span::styled(format!("{frame} {label}"), th.fg(th.accent)));
    }
    f.render_widget(Line::from(spans), area);
}

fn popup_area(area: Rect, width: u16, height: u16) -> Rect {
    let [v] = Layout::vertical([Constraint::Length(height.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [h] = Layout::horizontal([Constraint::Length(width.min(area.width))])
        .flex(Flex::Center)
        .areas(v);
    h
}

fn popup(f: &mut Frame, app: &App, title: String, lines: Vec<Line<'static>>, width: u16) {
    // Long lines wrap; count the rows they take (borders and padding use 4 columns).
    let inner = width.min(f.area().width).saturating_sub(4).max(1) as usize;
    let rows: usize = lines.iter().map(|l| l.width().max(1).div_ceil(inner)).sum();
    let height = (rows as u16 + 4).min(f.area().height.saturating_sub(2));
    let area = popup_area(f.area(), width, height);
    f.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(app.theme.fg(app.theme.accent))
        .title(title_span(app, title))
        .title_bottom(
            Line::styled(
                format!(" {} ", t("tui.press_any_key")),
                app.theme.fg(app.theme.dim),
            )
            .alignment(Alignment::Right),
        )
        .padding(ratatui::widgets::Padding::horizontal(1));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        area,
    );
}

fn selected(app: &App) -> Option<&Candidate> {
    app.report.as_ref()?.candidates.get(app.table.selected()?)
}

fn draw_details(f: &mut Frame, app: &App) {
    let Some(c) = selected(app) else { return };
    let th = &app.theme;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(fmt_bytes(c.bytes), th.bold(th.text)),
            Span::raw("  "),
            safety_dot(app, c),
            Span::raw(" "),
            Span::styled(
                if c.safety == Safety::Safe {
                    t("safety.safe")
                } else {
                    t("safety.review")
                },
                th.fg(if c.safety == Safety::Safe {
                    th.safe
                } else {
                    th.review
                }),
            ),
            Span::styled(format!("  ·  {}", c.category.render()), th.fg(th.dim)),
        ]),
        Line::styled(c.path.clone(), th.fg(th.dim)),
        Line::raw(""),
    ];
    if let Some(b) = c.frees_less() {
        lines.insert(
            1,
            Line::styled(
                tf("size.frees", &[("bytes", &fmt_bytes(b))]),
                th.fg(th.review),
            ),
        );
    }
    for e in &c.evidence {
        lines.push(Line::from(vec![
            Span::styled("✓ ", th.fg(th.safe)),
            Span::raw(e.render()),
        ]));
    }
    for n in &c.notes {
        lines.push(Line::from(vec![
            Span::styled("• ", th.fg(th.review)),
            Span::raw(n.render()),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("→ ", th.fg(th.accent)),
        Span::raw(describe(&c.action, false)),
    ]));
    lines.push(Line::styled(c.id.clone(), th.fg(th.dim)));
    popup(f, app, c.title.render(), lines, 90);
}

fn draw_confirm(f: &mut Frame, app: &App) {
    let Mode::Confirm(plan) = &app.mode else {
        return;
    };
    let th = &app.theme;
    let total: u64 = plan.items.iter().map(|(_, c)| c.frees()).sum();
    let mut lines = vec![];
    for (_, c) in &plan.items {
        let mut row = vec![
            Span::styled(format!("{:>9}  ", fmt_bytes(c.bytes)), th.bold(th.text)),
            Span::raw(c.title.render()),
        ];
        if let Some(b) = c.frees_less() {
            row.push(Span::styled(
                format!("  {}", tf("size.frees_short", &[("bytes", &fmt_bytes(b))])),
                th.fg(th.review),
            ));
        }
        lines.push(Line::from(row));
        lines.push(Line::styled(
            format!("           {}", describe(&c.action, false)),
            th.fg(th.dim),
        ));
    }
    for r in &plan.refused {
        let (title, why) = (&r.title, &r.reason);
        lines.push(Line::from(vec![
            Span::styled("       --  ", th.fg(th.review)),
            Span::raw(format!("{title}: {why}")),
        ]));
    }
    lines.push(Line::raw(""));
    if plan.items.is_empty() {
        lines.push(Line::styled(t("tui.nothing_to_do"), th.fg(th.review)));
    } else {
        lines.push(Line::from(vec![
            Span::styled(" y ", th.bold(th.danger).add_modifier(Modifier::REVERSED)),
            Span::styled(
                format!(
                    " {}   ",
                    tf("tui.confirm_yes", &[("bytes", &fmt_bytes(total))])
                ),
                th.fg(th.text),
            ),
            Span::styled(" n ", th.bold(th.dim).add_modifier(Modifier::REVERSED)),
            Span::styled(format!(" {}", t("tui.confirm_no")), th.fg(th.dim)),
        ]));
    }
    popup(f, app, t("clean.plan"), lines, 96);
}

fn draw_message(f: &mut Frame, app: &App, lines: Vec<String>) {
    let th = &app.theme;
    let styled = lines
        .into_iter()
        .map(|l| {
            let style = if l.starts_with('✓') {
                th.fg(th.safe)
            } else if l.starts_with('✗') {
                th.fg(th.danger)
            } else {
                th.fg(th.text)
            };
            Line::styled(l, style)
        })
        .collect();
    popup(f, app, t("tui.result"), styled, 80);
}

fn draw_help(f: &mut Frame, app: &App) {
    let th = &app.theme;
    let rows = [
        ("↑↓ j k", t("tui.key_move")),
        ("space", t("tui.key_mark")),
        ("a", t("tui.help_all_safe")),
        ("enter", t("tui.key_details")),
        ("c", t("tui.help_clean")),
        ("r", t("tui.key_rescan")),
        ("tab 1 2", t("tui.key_tab")),
        ("q esc", t("tui.key_quit")),
    ];
    let mut lines: Vec<Line> = rows
        .into_iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!("{k:>8}  "), th.bold(th.accent)),
                Span::raw(d),
            ])
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("● ", th.fg(th.safe)),
        Span::raw(t("tui.legend_safe")),
    ]));
    lines.push(Line::from(vec![
        Span::styled("○ ", th.fg(th.review)),
        Span::raw(t("tui.legend_review")),
    ]));
    lines.push(Line::raw(""));
    lines.push(Line::styled(t("tui.help_safety"), th.fg(th.dim)));
    lines.push(Line::styled(t("tui.help_frees"), th.fg(th.dim)));
    lines.push(Line::styled(t("tui.help_purgeable"), th.fg(th.dim)));
    popup(f, app, t("tui.help"), lines, 76);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn bar_widths_are_exact() {
        for ratio in [0.0, 0.01, 0.33, 0.5, 0.999, 1.0, 1.7, -1.0] {
            let s = text(&bar(ratio, 10, Style::new(), Style::new()));
            assert_eq!(s.chars().count(), 10, "ratio {ratio}: {s:?}");
        }
        assert_eq!(text(&bar(1.0, 4, Style::new(), Style::new())), "████");
        assert_eq!(text(&bar(0.0, 4, Style::new(), Style::new())), "░░░░");
    }

    #[test]
    fn long_paths_keep_their_end() {
        let s = short_path("/x/very/long/path/to/DerivedData", 12);
        assert!(
            s.starts_with('…') && s.ends_with("DerivedData") && s.chars().count() == 12,
            "{s}"
        );
    }
}
