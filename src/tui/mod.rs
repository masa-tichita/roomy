//! Interactive dashboard (`roomy` in a terminal, or `roomy ui`).
//!
//! Slow work (scanning, re-checking, removing) runs on a background thread so the screen stays
//! responsive. Removal goes through exactly the same plan / re-check / guard / audit path as
//! `roomy clean`.

mod theme;
mod view;

use crate::clean::{self, Outcome, Plan};
use crate::config::Config;
use crate::i18n::t;
use crate::model::{Action, Report, Safety};
use crate::state;
use crate::sys::{self, AppMem, Disk, Mem};
use crate::tick::Sample;
use crate::walk::Snapshot;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub use theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Overview,
    Folders,
}

pub enum Mode {
    Normal,
    Details,
    Help,
    /// Waiting for a background job; the text says what for.
    Working(String, Instant),
    Confirm(Plan),
    Message(Vec<String>),
}

enum Job {
    Scanned(Result<Report, String>),
    Planned(Plan),
    Cleaned(Result<(Outcome, Report), String>),
}

pub struct App {
    pub cfg: Config,
    pub theme: Theme,
    pub report: Option<Report>,
    pub disk: Disk,
    pub mem: Mem,
    pub apps: Vec<AppMem>,
    /// Lowest free space per day, oldest first.
    pub free_series: Vec<u64>,
    pub table: TableState,
    pub marked: BTreeSet<usize>,
    pub mode: Mode,
    pub tab: Tab,
    pub snap: Option<Snapshot>,
    pub prev_snap: Option<Snapshot>,
    pub folder: PathBuf,
    pub folder_table: TableState,
    jobs: Option<Receiver<Job>>,
    last_refresh: Instant,
    last_apps: Instant,
    quit: bool,
}

pub fn run(cfg: &Config) -> anyhow::Result<()> {
    let mut app = App::new(cfg.clone());
    let mut terminal = ratatui::init();
    let result = app.event_loop(&mut terminal);
    ratatui::restore();
    result
}

/// Renders one frame as plain text (no terminal needed). Used by tests and for screenshots in docs.
pub fn render_text(cfg: &Config, width: u16, height: u16, tab: Tab) -> String {
    let mut app = App::new(cfg.clone());
    app.tab = tab;
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height))
        .expect("test backend");
    let _ = terminal.draw(|f| view::draw(f, &mut app));
    let buf = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        let mut line = String::new();
        let mut x = 0;
        while x < buf.area.width {
            let sym = buf[(x, y)].symbol();
            line.push_str(sym);
            // A wide character occupies two cells; the second one is a placeholder.
            x += unicode_width::UnicodeWidthStr::width(sym).max(1) as u16;
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

impl App {
    fn new(cfg: Config) -> App {
        let report = state::load::<Report>(&state::report_path());
        let mut app = App {
            cfg,
            theme: Theme::detect(),
            report,
            disk: sys::disk(),
            mem: sys::mem(),
            apps: sys::top_apps(5),
            free_series: vec![],
            table: TableState::default().with_selected(Some(0)),
            marked: BTreeSet::new(),
            mode: Mode::Normal,
            tab: Tab::Overview,
            snap: None,
            prev_snap: None,
            folder: sys::home(),
            folder_table: TableState::default().with_selected(Some(0)),
            jobs: None,
            last_refresh: Instant::now(),
            last_apps: Instant::now(),
            quit: false,
        };
        app.load_history();
        app.load_snapshots();
        if app.report.is_none() {
            app.start_scan();
        }
        app
    }

    fn load_history(&mut self) {
        let samples: Vec<Sample> = state::read_lines(&state::history_path());
        let mut by_day = std::collections::BTreeMap::<String, u64>::new();
        for s in &samples {
            let day: String = sys::fmt_time(s.t).chars().take(10).collect();
            let e = by_day.entry(day).or_insert(u64::MAX);
            *e = (*e).min(s.avail);
        }
        self.free_series = by_day
            .into_values()
            .rev()
            .take(14)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        if self.free_series.len() < 2 {
            // Not enough days yet: use the most recent samples instead.
            self.free_series = samples
                .iter()
                .rev()
                .take(48)
                .map(|s| s.avail)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
        }
    }

    fn load_snapshots(&mut self) {
        let files = state::snapshot_files();
        self.snap = files.first().and_then(|p| state::load(p));
        self.prev_snap = match &self.snap {
            Some(cur) => files
                .iter()
                .skip(1)
                .filter_map(|p| state::load::<Snapshot>(p))
                .find(|s| s.comparable(cur)),
            None => None,
        };
    }

    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
        while !self.quit {
            terminal.draw(|f| view::draw(f, self))?;
            if event::poll(Duration::from_millis(200))?
                && let Event::Key(k) = event::read()?
                && k.kind == KeyEventKind::Press
            {
                self.on_key(k);
            }
            self.poll_jobs();
            self.tick();
        }
        Ok(())
    }

    fn tick(&mut self) {
        if self.last_refresh.elapsed() >= Duration::from_secs(2) {
            self.disk = sys::disk();
            self.mem = sys::mem();
            self.last_refresh = Instant::now();
        }
        if self.last_apps.elapsed() >= Duration::from_secs(5) {
            self.apps = sys::top_apps(5);
            self.last_apps = Instant::now();
        }
    }

    pub fn busy(&self) -> bool {
        self.jobs.is_some()
    }

    fn spawn(&mut self, label: String, f: impl FnOnce() -> Job + Send + 'static) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.jobs = Some(rx);
        self.mode = Mode::Working(label, Instant::now());
    }

    fn start_scan(&mut self) {
        if self.busy() {
            return;
        }
        let cfg = self.cfg.clone();
        self.spawn(t("tui.scanning"), move || {
            let Some(_lock) = sys::try_lock(&state::lock_path()) else {
                return Job::Scanned(Err(t("scan.locked")));
            };
            Job::Scanned(crate::scan::run(&cfg, false).map_err(|e| e.to_string()))
        });
    }

    fn start_plan(&mut self) {
        let Some(report) = self.report.clone() else {
            return;
        };
        let picked: Vec<usize> = if self.marked.is_empty() {
            self.table
                .selected()
                .into_iter()
                .filter(|i| *i < report.candidates.len())
                .collect()
        } else {
            self.marked.iter().copied().collect()
        };
        if picked.is_empty() || self.busy() {
            return;
        }
        let cfg = self.cfg.clone();
        self.spawn(t("tui.checking"), move || {
            Job::Planned(clean::prepare(
                &cfg,
                &report,
                &picked,
                clean::Policy::INTERACTIVE,
            ))
        });
    }

    fn start_clean(&mut self, plan: Plan) {
        let Some(mut report) = self.report.clone() else {
            return;
        };
        self.spawn(t("tui.cleaning"), move || {
            let Some(_lock) = sys::try_lock(&state::lock_path()) else {
                return Job::Cleaned(Err(t("scan.locked")));
            };
            Job::Cleaned(
                clean::apply(&mut report, &plan, false, true, false)
                    .map(|o| (o, report))
                    .map_err(|e| e.to_string()),
            )
        });
    }

    fn poll_jobs(&mut self) {
        let Some(rx) = &self.jobs else { return };
        let Ok(job) = rx.try_recv() else { return };
        self.jobs = None;
        self.mode = Mode::Normal;
        match job {
            Job::Scanned(Ok(r)) => {
                self.report = Some(r);
                self.marked.clear();
                self.table.select(Some(0));
                self.disk = sys::disk();
                self.load_snapshots();
            }
            Job::Planned(plan) => self.mode = Mode::Confirm(plan),
            Job::Cleaned(Ok((out, report))) => {
                let mut lines: Vec<String> = out
                    .results
                    .iter()
                    .map(|(title, r)| match r {
                        Ok(()) => format!("✓ {title}"),
                        Err(e) => format!("✗ {title}: {e}"),
                    })
                    .collect();
                lines.push(String::new());
                lines.push(crate::i18n::tf(
                    "clean.freed",
                    &[("bytes", &sys::fmt_bytes(out.freed))],
                ));
                if out.moved_to_trash {
                    lines.push(t("clean.trash_note"));
                }
                self.report = Some(report);
                self.marked.clear();
                self.clamp_selection();
                self.disk = sys::disk();
                self.mode = Mode::Message(lines);
            }
            Job::Scanned(Err(e)) | Job::Cleaned(Err(e)) => self.mode = Mode::Message(vec![e]),
        }
    }

    fn clamp_selection(&mut self) {
        let n = self.report.as_ref().map_or(0, |r| r.candidates.len());
        let i = self.table.selected().unwrap_or(0).min(n.saturating_sub(1));
        self.table.select(Some(i));
    }

    fn candidates_len(&self) -> usize {
        self.report.as_ref().map_or(0, |r| r.candidates.len())
    }

    fn on_key(&mut self, k: KeyEvent) {
        if k.modifiers.contains(KeyModifiers::CONTROL) && k.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        match &self.mode {
            Mode::Working(..) => {
                if matches!(k.code, KeyCode::Char('q')) {
                    self.quit = true;
                }
            }
            Mode::Details | Mode::Help | Mode::Message(_) => self.mode = Mode::Normal,
            Mode::Confirm(_) => match k.code {
                KeyCode::Char('y' | 'Y') => {
                    if let Mode::Confirm(plan) = std::mem::replace(&mut self.mode, Mode::Normal) {
                        if plan.items.is_empty() {
                            return;
                        }
                        self.start_clean(plan);
                    }
                }
                _ => self.mode = Mode::Normal,
            },
            Mode::Normal => self.on_normal_key(k.code),
        }
    }

    fn on_normal_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Char('?') => self.mode = Mode::Help,
            KeyCode::Char('r') => self.start_scan(),
            KeyCode::Tab | KeyCode::BackTab => {
                self.tab = if self.tab == Tab::Overview {
                    Tab::Folders
                } else {
                    Tab::Overview
                };
            }
            KeyCode::Char('1') => self.tab = Tab::Overview,
            KeyCode::Char('2') => self.tab = Tab::Folders,
            _ if self.tab == Tab::Folders => self.on_folder_key(code),
            KeyCode::Down | KeyCode::Char('j') => self.table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.table.select_previous(),
            KeyCode::Home | KeyCode::Char('g') => self.table.select_first(),
            KeyCode::End | KeyCode::Char('G') => self
                .table
                .select(Some(self.candidates_len().saturating_sub(1))),
            KeyCode::Char(' ') => {
                if let Some(i) = self.table.selected().filter(|i| *i < self.candidates_len()) {
                    if !self.marked.remove(&i) {
                        self.marked.insert(i);
                    }
                    self.table.select_next();
                }
            }
            KeyCode::Char('a') => {
                if let Some(r) = &self.report {
                    let safe: BTreeSet<usize> = r
                        .candidates
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| {
                            c.safety == Safety::Safe && !matches!(c.action, Action::Manual { .. })
                        })
                        .map(|(i, _)| i)
                        .collect();
                    self.marked = if self.marked == safe {
                        BTreeSet::new()
                    } else {
                        safe
                    };
                }
            }
            KeyCode::Enter => {
                if self.candidates_len() > 0 {
                    self.mode = Mode::Details;
                }
            }
            KeyCode::Char('c' | 'd') => self.start_plan(),
            _ => {}
        }
        self.clamp_selection();
    }

    fn on_folder_key(&mut self, code: KeyCode) {
        let rows = view::folder_rows(self);
        match code {
            KeyCode::Down | KeyCode::Char('j') => self.folder_table.select_next(),
            KeyCode::Up | KeyCode::Char('k') => self.folder_table.select_previous(),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Some(row) = self.folder_table.selected().and_then(|i| rows.get(i))
                    && row.has_children
                {
                    self.folder = PathBuf::from(&row.path);
                    self.folder_table.select(Some(0));
                }
            }
            KeyCode::Left | KeyCode::Backspace | KeyCode::Char('h') => {
                let child = self.folder.clone();
                if let Some(parent) = child.parent()
                    && self
                        .snap
                        .as_ref()
                        .is_some_and(|s| s.entries.contains_key(&*parent.to_string_lossy()))
                {
                    self.folder = parent.to_path_buf();
                    let rows = view::folder_rows(self);
                    let idx = rows
                        .iter()
                        .position(|r| Path::new(&r.path) == child)
                        .unwrap_or(0);
                    self.folder_table.select(Some(idx));
                }
            }
            _ => {}
        }
        let n = view::folder_rows(self).len();
        let i = self
            .folder_table
            .selected()
            .unwrap_or(0)
            .min(n.saturating_sub(1));
        self.folder_table.select(Some(i));
    }
}
