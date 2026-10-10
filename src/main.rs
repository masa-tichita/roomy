mod api;
mod app;
mod clean;
mod config;
mod detect;
mod doctor;
mod guard;
mod guide;
mod i18n;
mod mcp;
mod memory;
mod model;
mod output;
mod rules;
mod scan;
mod service;
mod state;
mod sys;
mod tick;
mod tui;
mod walk;

use clap::{CommandFactory, Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "roomy",
    version,
    about = "Keeps your Mac roomy: finds what is safe to delete, warns before storage or memory runs out."
)]
struct Cli {
    /// Without a command: the dashboard in a terminal, `status` otherwise.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Interactive dashboard
    #[command(visible_alias = "tui", alias = "dashboard")]
    Ui {
        /// Print one frame as text (`WIDTHxHEIGHT`) instead of running interactively
        #[arg(long, hide = true, value_name = "WxH")]
        print: Option<String>,
        /// With --print: show the folders tab
        #[arg(long, hide = true)]
        folders: bool,
        /// With --print: show the memory tab
        #[arg(long, hide = true)]
        memory: bool,
    },
    /// Free space, memory pressure, swap, top memory users, last scan
    Status {
        #[arg(long)]
        json: bool,
        /// Exit with 3 when free space or memory is over a threshold
        #[arg(long)]
        exit_code: bool,
    },
    /// Walk the disk, find removal candidates and what grew since last time
    #[command(alias = "inspect")]
    Scan {
        #[arg(long)]
        json: bool,
        /// With --json: every field and path instead of the summary (large)
        #[arg(long)]
        full: bool,
        /// Send a notification with the summary
        #[arg(long)]
        notify: bool,
    },
    /// Show the last scan without scanning again
    Report {
        #[arg(long)]
        json: bool,
        /// With --json: every field and path instead of the summary (large)
        #[arg(long)]
        full: bool,
        /// Also show ids, paths and why worktrees were left out
        #[arg(short, long)]
        verbose: bool,
    },
    /// Remove candidates by number or id (re-checked right before removal)
    #[command(alias = "evict")]
    Clean {
        targets: Vec<String>,
        /// Every candidate marked safe
        #[arg(long)]
        all_safe: bool,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
        /// Show what would happen and stop
        #[arg(short = 'n', long)]
        dry_run: bool,
        /// Move files to the Trash instead of deleting them
        #[arg(long)]
        trash: bool,
        /// Unattended runs: also remove candidates marked "review"
        #[arg(long)]
        allow_review: bool,
        /// Machine-readable plan or result. Without --yes this is a dry run.
        #[arg(long)]
        json: bool,
    },
    /// One candidate in detail: evidence, caveats, the paths or command it involves
    Explain {
        /// Candidate number or id
        target: String,
        #[arg(long)]
        json: bool,
        /// How many paths to list
        #[arg(long, default_value_t = 20)]
        max_paths: usize,
    },
    /// How AI agents should use roomy (also as a Claude Code skill or AGENTS.md snippet)
    Guide {
        #[arg(long, value_enum, default_value = "text")]
        format: guide::Format,
        /// Install the Claude Code skill to ~/.claude/skills/roomy/SKILL.md
        #[arg(long)]
        install_claude_skill: bool,
        /// With --install-claude-skill: overwrite an existing file
        #[arg(long)]
        force: bool,
    },
    /// Model Context Protocol server on stdio, for AI agents
    Mcp {
        /// Do not offer the removal tool
        #[arg(long)]
        read_only: bool,
    },
    /// Swap, and the apps holding compressed or swapped memory; quit them to free it
    #[command(alias = "mem")]
    Memory {
        #[command(subcommand)]
        action: Option<MemoryCmd>,
        #[arg(long)]
        json: bool,
        /// How many to list
        #[arg(short, default_value_t = 15)]
        n: usize,
    },
    /// Largest folders from the last scan (no disk walk)
    Top {
        path: Option<String>,
        #[arg(short, default_value_t = 20)]
        n: usize,
        #[arg(long)]
        json: bool,
    },
    /// Lowest free space per day
    History {
        #[arg(long, default_value_t = 14)]
        days: usize,
        #[arg(long)]
        json: bool,
    },
    /// Periodic check, run by launchd every few minutes
    #[command(hide = true)]
    Tick,
    /// Install or remove the background service (not needed with `brew services`)
    Service {
        #[command(subcommand)]
        action: ServiceCmd,
    },
    /// Diagnose permissions, tools, configuration and the background service
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Configuration file
    Config {
        #[command(subcommand)]
        action: ConfigCmd,
    },
    /// List rules (built-in and your own)
    Rules {
        #[arg(long)]
        json: bool,
    },
    /// Shell completion script on stdout, e.g. `roomy completions zsh > _roomy`
    #[command(hide = true)]
    Completions { shell: clap_complete::Shell },
    /// Manual page (roff) on stdout, or one page per command into a folder
    #[command(hide = true)]
    Man {
        /// Write roomy.1 and roomy-<command>.1 into this folder instead
        #[arg(long, value_name = "DIR")]
        dir: Option<PathBuf>,
    },
}

impl Cmd {
    /// Output meant for programs: JSON, or the MCP protocol.
    fn machine_output(&self) -> bool {
        match self {
            Cmd::Status { json, .. }
            | Cmd::Scan { json, .. }
            | Cmd::Report { json, .. }
            | Cmd::Clean { json, .. }
            | Cmd::Explain { json, .. }
            | Cmd::Top { json, .. }
            | Cmd::History { json, .. }
            | Cmd::Doctor { json }
            | Cmd::Rules { json }
            | Cmd::Memory { json, .. } => *json,
            Cmd::Mcp { .. } => true,
            _ => false,
        }
    }
}

#[derive(Subcommand)]
enum MemoryCmd {
    /// Quit apps or processes by number, name or id (asks first; apps get the normal Quit)
    Quit {
        targets: Vec<String>,
        /// Do not ask for confirmation
        #[arg(short, long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum ServiceCmd {
    Install {
        /// Install even from a development build or a Homebrew install
        #[arg(long)]
        force: bool,
    },
    Uninstall {
        /// Also delete reports, snapshots and history
        #[arg(long)]
        purge: bool,
    },
    Status,
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print the path of the configuration file
    Path,
    /// Print the effective configuration
    Show,
    /// Write a commented starter file (keeps an existing one)
    Init,
    /// Check the file for errors and unknown keys
    Validate,
}

fn main() -> ExitCode {
    // Rust ignores SIGPIPE, so writing to a closed pipe (`roomy … | head`) would panic.
    // Restore the default like other Unix tools: the process ends quietly.
    // SAFETY: called first in main, before any thread exists; SIG_DFL is a valid disposition.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let cli = Cli::parse();
    let cmd = cli.cmd.unwrap_or_else(|| {
        use std::io::IsTerminal;
        if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
            Cmd::Ui {
                print: None,
                folders: false,
                memory: false,
            }
        } else {
            Cmd::Status {
                json: false,
                exit_code: false,
            }
        }
    });
    let loaded = config::load();
    // Machine output is always English, so it does not depend on who runs it.
    let machine = cmd.machine_output();
    i18n::init(if machine {
        "en"
    } else {
        &loaded.config.language
    });
    if let Some(e) = &loaded.error
        && !matches!(
            cmd,
            Cmd::Config { .. }
                | Cmd::Doctor { .. }
                | Cmd::Ui { .. }
                | Cmd::Completions { .. }
                | Cmd::Man { .. }
        )
    {
        eprintln!(
            "{}",
            i18n::tf(
                "config.broken",
                &[("path", &config::path().display()), ("error", e)]
            )
        );
    }
    match run(cmd, &loaded.config) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            if machine {
                output::print_json(&api::error(&format!("{e:#}")));
            } else {
                eprintln!("{}: {e:#}", app::NAME);
            }
            ExitCode::from(app::EXIT_ERROR)
        }
    }
}

fn run(cmd: Cmd, cfg: &config::Config) -> anyhow::Result<u8> {
    use i18n::{t, tf};
    match cmd {
        Cmd::Ui {
            print: Some(size),
            folders,
            memory,
        } => {
            let (w, h) = size
                .split_once('x')
                .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
                .unwrap_or((120, 36));
            let tab = if memory {
                tui::Tab::Memory
            } else if folders {
                tui::Tab::Folders
            } else {
                tui::Tab::Overview
            };
            print!("{}", tui::render_text(cfg, w, h, tab));
            Ok(0)
        }
        Cmd::Ui { print: None, .. } => {
            tui::run(cfg)?;
            Ok(0)
        }
        Cmd::Status { json, exit_code } => {
            let alarming = output::status(cfg, json);
            Ok(if exit_code && alarming {
                app::EXIT_THRESHOLD
            } else {
                0
            })
        }
        Cmd::Scan { json, full, notify } => {
            let Some(_lock) = sys::try_lock(&state::lock_path()) else {
                anyhow::bail!(t("scan.locked"))
            };
            if !json {
                eprintln!("{}", t("scan.walking"));
            }
            let r = scan::run(cfg, false)?;
            if json {
                output::print_json(&if full {
                    api::full_report(&r)
                } else {
                    api::summary(&r)
                });
            } else {
                output::report(&r, false);
            }
            if notify && let Some(body) = tick::daily_summary(cfg, &r) {
                sys::notify(&t("tick.daily_title"), &body);
            }
            Ok(0)
        }
        Cmd::Report {
            json,
            full,
            verbose,
        } => {
            let Some(r) = state::load::<model::Report>(&state::report_path()) else {
                anyhow::bail!(t("clean.no_report"))
            };
            if json {
                output::print_json(&if full {
                    api::full_report(&r)
                } else {
                    api::summary(&r)
                });
            } else {
                output::report(&r, verbose);
            }
            Ok(0)
        }
        Cmd::Clean {
            targets,
            all_safe,
            yes,
            dry_run,
            trash,
            allow_review,
            json,
        } => {
            let Some(_lock) = sys::try_lock(&state::lock_path()) else {
                anyhow::bail!(t("scan.locked"))
            };
            let code = clean::run(
                cfg,
                &clean::Opts {
                    targets,
                    all_safe,
                    yes,
                    dry_run,
                    trash,
                    allow_review,
                    json,
                },
            )?;
            Ok(code as u8)
        }
        Cmd::Explain {
            target,
            json,
            max_paths,
        } => {
            let Some(r) = state::load::<model::Report>(&state::report_path()) else {
                anyhow::bail!(t("clean.no_report"))
            };
            let v = api::explain(&r, &target, max_paths).map_err(anyhow::Error::msg)?;
            if json {
                output::print_json(&v);
            } else {
                output::explain(&r, &target, max_paths);
            }
            Ok(0)
        }
        Cmd::Guide {
            format,
            install_claude_skill,
            force,
        } => {
            if install_claude_skill {
                let path = guide::install_claude_skill(force)?;
                println!("{}", tf("guide.installed", &[("path", &path.display())]));
            } else {
                print!("{}", guide::render(format));
            }
            Ok(0)
        }
        Cmd::Mcp { read_only } => {
            mcp::serve(cfg, read_only)?;
            Ok(0)
        }
        Cmd::Memory {
            action: None,
            json,
            n,
        } => {
            let holders = memory::holders(n);
            let outside = memory::outside_space();
            if json {
                output::print_json(&api::memory(&holders, &outside));
            } else {
                output::memory(&holders, &outside);
            }
            Ok(0)
        }
        Cmd::Memory {
            action: Some(MemoryCmd::Quit { targets, yes }),
            ..
        } => quit_memory_holders(&targets, yes),
        Cmd::Top { path, n, json } => {
            let files = state::snapshot_files();
            let Some(snap) = files.first().and_then(|p| state::load::<walk::Snapshot>(p)) else {
                anyhow::bail!(t("clean.no_report"))
            };
            let prev = files
                .iter()
                .skip(1)
                .filter_map(|p| state::load::<walk::Snapshot>(p))
                .find(|s| s.comparable(&snap));
            let dir = match path {
                Some(p) => {
                    let p = sys::expand(&p);
                    std::fs::canonicalize(&p).unwrap_or(p)
                }
                None => sys::home(),
            };
            output::top(&snap, prev.as_ref(), &dir, n, json);
            Ok(0)
        }
        Cmd::History { days, json } => {
            output::history(&state::read_lines(&state::history_path()), days, json);
            Ok(0)
        }
        Cmd::Tick => {
            tick::run(cfg)?;
            Ok(0)
        }
        Cmd::Service { action } => {
            match action {
                ServiceCmd::Install { force } => service::install(cfg, force)?,
                ServiceCmd::Uninstall { purge } => {
                    service::uninstall()?;
                    if purge {
                        let dir = app::state_dir();
                        if dir.exists() {
                            std::fs::remove_dir_all(&dir)?;
                        }
                        println!("{}", tf("service.purged", &[("path", &dir.display())]));
                    }
                }
                ServiceCmd::Status => println!("{}", service::status_line()),
            }
            Ok(0)
        }
        Cmd::Doctor { json } => Ok(if doctor::run(json) > 0 {
            app::EXIT_ERROR
        } else {
            0
        }),
        Cmd::Config { action } => {
            let path = config::path();
            match action {
                ConfigCmd::Path => println!("{}", path.display()),
                ConfigCmd::Show => print!("{}", toml::to_string_pretty(cfg)?),
                ConfigCmd::Init => {
                    if path.exists() {
                        println!("{}", tf("config.exists", &[("path", &path.display())]));
                    } else {
                        std::fs::create_dir_all(app::config_dir())?;
                        std::fs::write(&path, config::template())?;
                        println!("{}", tf("config.created", &[("path", &path.display())]));
                    }
                }
                ConfigCmd::Validate => {
                    let loaded = config::load();
                    if let Some(e) = &loaded.error {
                        println!(
                            "{}",
                            tf("config.broken", &[("path", &path.display()), ("error", e)])
                        );
                        return Ok(1);
                    }
                    let mut bad = false;
                    for k in &loaded.unknown_keys {
                        println!("{}", tf("config.unknown_key", &[("key", k)]));
                    }
                    for e in rules::validate(&rules::all(&loaded.config)) {
                        bad = true;
                        println!("{e}");
                    }
                    if !bad && loaded.unknown_keys.is_empty() {
                        println!("{}", t("config.valid"));
                    }
                    return Ok(u8::from(bad));
                }
            }
            Ok(0)
        }
        Cmd::Rules { json } => {
            output::rules(cfg, json);
            Ok(0)
        }
        Cmd::Completions { shell } => {
            completions(shell);
            Ok(0)
        }
        Cmd::Man { dir } => {
            man(dir.as_deref())?;
            Ok(0)
        }
    }
}

/// Completions and manual pages come from the command definitions alone, so packaging (the
/// Homebrew formula) can generate them at install time.
fn completions(shell: clap_complete::Shell) {
    // clap_complete would also offer hidden commands (`tick`, `completions`, `man`).
    let full = Cli::command();
    let mut cmd = clap::Command::new(app::NAME)
        .version(app::VERSION)
        .args(full.get_arguments().cloned())
        .subcommands(full.get_subcommands().filter(|s| !s.is_hide_set()).cloned());
    clap_complete::generate(shell, &mut cmd, app::NAME, &mut std::io::stdout());
}

/// `roomy.1` on stdout, or with `dir`, `roomy.1` and `roomy-<command>.1` for every visible
/// command (the pages `roomy.1` refers to) into that folder.
fn man(dir: Option<&Path>) -> std::io::Result<()> {
    match dir {
        Some(dir) => {
            std::fs::create_dir_all(dir)?;
            clap_mangen::generate_to(Cli::command(), dir)
        }
        None => clap_mangen::Man::new(Cli::command()).render(&mut std::io::stdout()),
    }
}

/// `roomy memory quit`: resolve targets against a fresh listing, confirm, quit.
fn quit_memory_holders(targets: &[String], yes: bool) -> anyhow::Result<u8> {
    use i18n::{t, tf};
    use std::io::{BufRead, IsTerminal, Write};
    if targets.is_empty() {
        anyhow::bail!(t("memory.nothing"));
    }
    let holders = memory::holders(100);
    let mut picked: Vec<&memory::Holder> = vec![];
    for tg in targets {
        let found = match tg.parse::<usize>() {
            Ok(n) if n >= 1 => holders.get(n - 1),
            _ => holders
                .iter()
                .find(|h| h.id == *tg || h.name.eq_ignore_ascii_case(tg)),
        };
        let h =
            found.ok_or_else(|| anyhow::anyhow!(tf("memory.unknown_target", &[("target", tg)])))?;
        if let Some(why) = &h.protected {
            anyhow::bail!("{}: {}", h.name, why.render());
        }
        if !picked.iter().any(|p| p.id == h.id) {
            picked.push(h);
        }
    }
    println!("{}", t("memory.quit_plan"));
    for h in &picked {
        let what = h
            .detail
            .clone()
            .unwrap_or_else(|| h.pids.len().to_string() + " processes");
        println!(
            "  {}  {}  {}",
            sys::pad_left(&sys::fmt_mem(h.compressed), 8),
            h.name,
            what
        );
    }
    if !yes {
        if !std::io::stdin().is_terminal() {
            anyhow::bail!(t("clean.need_yes"));
        }
        print!("{} ", t("memory.quit_confirm"));
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if !matches!(line.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("{}", t("clean.cancelled"));
            return Ok(1);
        }
    }
    let mut ok = true;
    for h in picked {
        match memory::quit(h) {
            memory::QuitOutcome::Quit => {
                println!("  ✓ {}", tf("memory.quit_done", &[("name", &h.name)]));
            }
            memory::QuitOutcome::Asked => {
                println!("  … {}", tf("memory.quit_asked", &[("name", &h.name)]));
            }
            memory::QuitOutcome::Failed(e) => {
                ok = false;
                println!("  ✗ {}: {e}", h.name);
            }
        }
    }
    println!(
        "{}",
        tf(
            "memory.swap_now",
            &[("swap", &sys::fmt_mem(sys::mem().swap_used))]
        )
    );
    Ok(if ok { 0 } else { app::EXIT_ERROR })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_definitions_are_consistent() {
        Cli::command().debug_assert();
    }
}
