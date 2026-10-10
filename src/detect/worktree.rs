//! Git worktrees that are finished with: clean, pushed, idle, not in use.

use super::Ctx;
use crate::i18n::{Msg, m, mf};
use crate::model::{Action, Candidate, Excluded, Safety};
use crate::sys::{days_since, expand, run, tilde};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Folders never descended into while looking for worktrees.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    ".venv",
    "venv",
    "build",
    "dist",
    "Library",
    ".next",
    "Pods",
    "DerivedData",
    ".terraform",
    ".gradle",
    "vendor",
];

/// Ignored entries that are rebuilt by tooling, so losing them is fine.
const REGENERABLE: &[&str] = &[
    "node_modules",
    ".venv",
    "venv",
    "env",
    "__pycache__",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".cache",
    "dist",
    "build",
    "out",
    "target",
    ".gradle",
    "Pods",
    "DerivedData",
    ".build",
    ".swiftpm",
    "xcuserdata",
    "coverage",
    ".nyc_output",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".tox",
    ".terraform",
    ".dart_tool",
    ".expo",
    ".DS_Store",
    ".eslintcache",
    "next-env.d.ts",
    ".vercel",
    ".angular",
    "storybook-static",
];
const REGENERABLE_EXT: &[&str] = &["pyc", "log", "tsbuildinfo", "o", "class"];

const MAX_DEPTH: usize = 5;

pub fn detect(ctx: &Ctx, out: &mut Vec<Candidate>, excluded: &mut Vec<Excluded>) {
    let mut found = vec![];
    for root in &ctx.cfg.worktrees.roots {
        let root = expand(root);
        if root.is_dir() && !ctx.is_off_limits(&root) {
            find(&root, 0, &mut found);
        }
    }
    found.sort();
    found.dedup();
    for w in found {
        match evaluate(ctx, &w) {
            Ok(c) => out.push(c),
            Err(reason) => excluded.push(Excluded {
                path: tilde(&w),
                reason,
            }),
        }
    }
}

pub fn recheck(ctx: &Ctx, c: &Candidate) -> Result<Candidate, Msg> {
    evaluate(ctx, &expand(&c.path))
}

pub fn find(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = e.file_name();
        if SKIP_DIRS.contains(&&*name.to_string_lossy()) {
            continue;
        }
        let p = e.path();
        if is_linked_worktree(&p) {
            out.push(p);
            continue;
        }
        find(&p, depth + 1, out);
    }
}

/// A linked worktree has a `.git` *file* pointing into `<repo>/.git/worktrees/<name>`.
/// (Submodules also have a `.git` file, pointing into `.git/modules/`.)
pub fn is_linked_worktree(p: &Path) -> bool {
    fs::read_to_string(p.join(".git"))
        .is_ok_and(|s| s.starts_with("gitdir:") && s.contains("/worktrees/"))
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let mut argv: Vec<&str> = vec!["git", "-C", dir.to_str()?];
    argv.extend_from_slice(args);
    run(&argv, None, Duration::from_secs(60))
        .filter(|o| o.ok)
        .map(|o| o.stdout)
}

pub struct Status {
    pub dirty: usize,
    pub ignored: Vec<String>,
}

/// Parses `git status --porcelain=v1 -z --ignored=matching`.
pub fn parse_status(s: &str) -> Status {
    let mut dirty = 0;
    let mut ignored = vec![];
    let mut fields = s.split('\0').filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        if f.len() < 4 {
            continue;
        }
        let (xy, path) = f.split_at(2);
        let path = &path[1..];
        if xy == "!!" {
            ignored.push(path.to_string());
        } else {
            dirty += 1;
            // Renames and copies carry the original path as an extra field.
            if xy.contains('R') || xy.contains('C') {
                fields.next();
            }
        }
    }
    Status { dirty, ignored }
}

/// Whether an ignored entry (path relative to the worktree) is rebuilt by tooling.
pub fn is_regenerable(entry: &str) -> bool {
    let trimmed = entry.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    trimmed.split('/').any(|c| REGENERABLE.contains(&c))
        || name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| REGENERABLE_EXT.contains(&ext))
        || name.starts_with(".env") && name.ends_with(".example")
        || name == "CACHEDIR.TAG"
}

/// Inside a folder marked with `CACHEDIR.TAG` (the cross-tool convention for caches).
pub fn in_tagged_cache(worktree: &Path, entry: &str) -> bool {
    let mut dir = worktree.join(entry.trim_end_matches('/'));
    if !dir.is_dir() {
        dir.pop();
    }
    while dir.starts_with(worktree) && dir != worktree {
        if dir.join("CACHEDIR.TAG").is_file() {
            return true;
        }
        dir.pop();
    }
    false
}

pub fn evaluate(ctx: &Ctx, w: &Path) -> Result<Candidate, Msg> {
    if !is_linked_worktree(w) {
        return Err(m("recheck.gone"));
    }
    if ctx.cwds().iter().any(|c| c.starts_with(w)) {
        return Err(m("wt.in_use"));
    }
    let status = git(
        w,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored=matching",
            "--untracked-files=normal",
        ],
    )
    .map(|s| parse_status(&s))
    .ok_or_else(|| m("wt.git_failed"))?;
    if status.dirty > 0 {
        return Err(mf("wt.dirty", &[("n", &status.dirty)]));
    }

    let mut evidence = vec![m("wt.clean")];
    let branch = git(w, &["branch", "--show-current"])
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    evidence.insert(
        0,
        if branch.is_empty() {
            m("wt.detached")
        } else {
            mf("wt.branch", &[("name", &branch)])
        },
    );

    match git(w, &["rev-list", "--count", "@{u}..HEAD"]).and_then(|s| s.trim().parse::<u64>().ok())
    {
        Some(0) => evidence.push(m("wt.pushed")),
        Some(n) => return Err(mf("wt.unpushed", &[("n", &n)])),
        None => {
            let on_remote = git(w, &["branch", "-r", "--contains", "HEAD"])
                .is_some_and(|s| !s.trim().is_empty());
            if !on_remote {
                return Err(m("wt.not_on_remote"));
            }
            evidence.push(m("wt.on_remote"));
        }
    }

    let reflog = git(w, &["reflog", "-1", "--date=unix", "--format=%gd"]).and_then(|s| {
        s.trim()
            .split_once('{')
            .and_then(|(_, r)| r.trim_end_matches('}').parse::<i64>().ok())
    });
    let commit = git(w, &["log", "-1", "--format=%ct"]).and_then(|s| s.trim().parse::<i64>().ok());
    let last = reflog.into_iter().chain(commit).max().unwrap_or(0);
    let idle = days_since(last);
    if idle < ctx.cfg.worktrees.idle_days {
        return Err(mf("wt.recent", &[("days", &idle)]));
    }
    evidence.push(mf("wt.idle", &[("days", &idle)]));

    if let Some(default) =
        git(w, &["rev-parse", "--abbrev-ref", "origin/HEAD"]).map(|s| s.trim().to_string())
        && !default.is_empty()
        && git(w, &["merge-base", "--is-ancestor", "HEAD", &default]).is_some()
    {
        evidence.push(mf("wt.merged", &[("branch", &default)]));
    }

    let main = git(
        w,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(|s| PathBuf::from(s.trim()))
    .and_then(|gd| gd.parent().map(Path::to_path_buf))
    .ok_or_else(|| m("wt.git_failed"))?;

    let unique: Vec<&String> = status
        .ignored
        .iter()
        .filter(|e| !is_regenerable(e) && !in_tagged_cache(w, e))
        .collect();
    let mut notes = vec![m("wt.branch_kept")];
    let mut safety = Safety::Safe;
    if !unique.is_empty() {
        safety = Safety::Review;
        let list: Vec<&str> = unique.iter().take(5).map(|s| s.as_str()).collect();
        let note = if unique.len() > 5 {
            mf(
                "wt.unique_files_more",
                &[("files", &list.join(", ")), ("n", &(unique.len() - 5))],
            )
        } else {
            mf("wt.unique_files", &[("files", &list.join(", "))])
        };
        notes.insert(0, note);
    }
    if let Some(m) = ctx
        .cfg
        .worktrees
        .managed
        .iter()
        .find(|m| w.starts_with(expand(&m.path)))
    {
        safety = Safety::Review;
        notes.push(mf("wt.managed", &[("tool", &m.name)]));
    }

    let name = w
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let repo = main
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(Candidate {
        id: format!("worktree:{}", tilde(w)),
        source: "worktree".into(),
        path: tilde(w),
        category: m("category.worktree"),
        title: mf("wt.title", &[("repo", &repo), ("name", &name)]),
        bytes: ctx.sizer.size(w),
        safety,
        evidence,
        notes,
        action: Action::Command {
            argv: [
                "git",
                "-C",
                &main.to_string_lossy(),
                "worktree",
                "remove",
                &w.to_string_lossy(),
            ]
            .iter()
            .map(ToString::to_string)
            .collect(),
            cwd: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::walk::{Sizer, Snapshot};
    use std::process::Command;

    #[test]
    fn porcelain_parsing() {
        let s = " M src/a.rs\0R  new.rs\0old.rs\0!! node_modules/\0!! .env\0?? notes.txt\0";
        let st = parse_status(s);
        assert_eq!(st.dirty, 3);
        assert_eq!(st.ignored, vec!["node_modules/", ".env"]);
    }

    #[test]
    fn regenerable_entries() {
        assert!(is_regenerable("web/node_modules/"));
        assert!(is_regenerable("app.pyc"));
        assert!(is_regenerable(".env.example"));
        assert!(!is_regenerable(".env"));
        assert!(!is_regenerable(".env.local"));
        assert!(!is_regenerable("bench/work/"));
        assert!(is_regenerable("web/node_modules/.cache/x"));
    }

    #[test]
    fn cachedir_tag_marks_a_cache() {
        let d = tempfile::tempdir().unwrap();
        let w = d.path();
        fs::create_dir_all(w.join("backend/cache/ruff/0.14")).unwrap();
        fs::write(
            w.join("backend/cache/ruff/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55",
        )
        .unwrap();
        fs::write(w.join("backend/cache/ruff/0.14/x"), "1").unwrap();
        assert!(in_tagged_cache(w, "backend/cache/ruff/0.14/"));
        assert!(in_tagged_cache(w, "backend/cache/ruff/0.14/x"));
        assert!(!in_tagged_cache(w, "backend/notes.md"));
    }

    fn sh(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .env("GIT_AUTHOR_DATE", "2020-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2020-01-01T00:00:00Z")
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    /// origin (bare) <- repo, with a linked worktree on a pushed branch.
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().canonicalize().unwrap();
        let origin = root.join("origin.git");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        sh(
            &root,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        sh(&repo, &["init", "-q", "-b", "main"]);
        fs::write(repo.join(".gitignore"), "node_modules/\n.env\nwork/\n").unwrap();
        fs::write(repo.join("a.txt"), "a").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "init"]);
        sh(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        sh(&repo, &["push", "-q", "-u", "origin", "main"]);
        let wt = root.join("wt");
        sh(
            &repo,
            &["worktree", "add", "-q", "-b", "feat", wt.to_str().unwrap()],
        );
        fs::write(wt.join("b.txt"), "b").unwrap();
        sh(&wt, &["add", "."]);
        sh(&wt, &["commit", "-q", "-m", "feat"]);
        sh(&wt, &["push", "-q", "-u", "origin", "feat"]);
        (d, repo, wt)
    }

    fn ctx_with(cfg: &Config, sizer: &Sizer, cwds: Vec<PathBuf>) -> Ctx<'static> {
        // Tests leak the config and sizer to get 'static references; fine for short-lived tests.
        let cfg: &'static Config = Box::leak(Box::new(cfg.clone()));
        let sizer: &'static Sizer = Box::leak(Box::new(Sizer {
            snap: sizer.snap.clone(),
        }));
        Ctx {
            cfg,
            sizer,
            cmdlines: vec![],
            cwds: cwds.into(),
            off_limits: vec![],
        }
    }

    fn base() -> (Config, Sizer) {
        let mut cfg = Config::default();
        cfg.worktrees.idle_days = 7; // commits are dated 2020, so they are idle
        (
            cfg,
            Sizer {
                snap: Snapshot::default(),
            },
        )
    }

    #[test]
    fn finished_worktree_is_safe() {
        let (_d, _repo, wt) = setup();
        let (cfg, sizer) = base();
        let c = evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).unwrap();
        assert_eq!(c.safety, Safety::Safe);
        assert!(
            matches!(&c.action, Action::Command { argv, .. } if argv.contains(&"worktree".to_string()))
        );
    }

    #[test]
    fn found_from_root_but_main_repo_is_not() {
        let (d, _repo, wt) = setup();
        let mut v = vec![];
        find(&d.path().canonicalize().unwrap(), 0, &mut v);
        assert_eq!(v, vec![wt]);
    }

    #[test]
    fn uncommitted_change_excludes() {
        let (_d, _repo, wt) = setup();
        fs::write(wt.join("b.txt"), "changed").unwrap();
        let (cfg, sizer) = base();
        let err = evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).unwrap_err();
        assert_eq!(err, mf("wt.dirty", &[("n", &1)]));
    }

    #[test]
    fn untracked_file_excludes() {
        let (_d, _repo, wt) = setup();
        fs::write(wt.join("new.txt"), "x").unwrap();
        let (cfg, sizer) = base();
        assert!(evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).is_err());
    }

    #[test]
    fn unpushed_commit_excludes() {
        let (_d, _repo, wt) = setup();
        fs::write(wt.join("c.txt"), "c").unwrap();
        sh(&wt, &["add", "."]);
        sh(&wt, &["commit", "-q", "-m", "local"]);
        let (cfg, sizer) = base();
        let err = evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).unwrap_err();
        assert_eq!(err, mf("wt.unpushed", &[("n", &1)]));
    }

    #[test]
    fn in_use_excludes() {
        let (_d, _repo, wt) = setup();
        let (cfg, sizer) = base();
        let err = evaluate(&ctx_with(&cfg, &sizer, vec![wt.join("sub")]), &wt).unwrap_err();
        assert_eq!(err, m("wt.in_use"));
    }

    #[test]
    fn unique_ignored_files_need_review() {
        let (_d, _repo, wt) = setup();
        fs::create_dir_all(wt.join("node_modules/x")).unwrap();
        fs::write(wt.join("node_modules/x/i.js"), "1").unwrap();
        let (cfg, sizer) = base();
        let c = evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).unwrap();
        assert_eq!(c.safety, Safety::Safe, "node_modules alone is regenerable");

        fs::write(wt.join(".env"), "SECRET=1").unwrap();
        fs::create_dir_all(wt.join("work")).unwrap();
        fs::write(wt.join("work/result.csv"), "1").unwrap();
        let c = evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).unwrap();
        assert_eq!(c.safety, Safety::Review);
        let note = c.notes[0].render();
        assert!(note.contains(".env") && note.contains("work/"), "{note}");
    }

    #[test]
    fn recently_used_excludes() {
        let (_d, _repo, wt) = setup();
        let (mut cfg, sizer) = base();
        cfg.worktrees.idle_days = 100_000;
        assert!(evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).is_err());
    }

    #[test]
    fn managed_folder_needs_review() {
        let (d, _repo, wt) = setup();
        let (mut cfg, sizer) = base();
        cfg.worktrees.managed.push(crate::config::Managed {
            path: d
                .path()
                .canonicalize()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            name: "Orca".into(),
        });
        let c = evaluate(&ctx_with(&cfg, &sizer, vec![]), &wt).unwrap();
        assert_eq!(c.safety, Safety::Review);
    }
}
