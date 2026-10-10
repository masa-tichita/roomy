//! The last line of defence before anything is removed.
//!
//! A path may be removed only if it is inside the home folder, at least two levels deep,
//! not a protected folder or an ancestor of one, and reached without symbolic links.

use crate::sys::{expand, home};
use std::path::{Component, Path, PathBuf};

/// Never removed, and nothing inside them either.
const PROTECTED_TREES: &[&str] = &[
    "~/Desktop",
    "~/Documents",
    "~/Pictures",
    "~/Movies",
    "~/Music",
    "~/.ssh",
    "~/.gnupg",
    "~/.config",
    "~/Library/Keychains",
    "~/Library/Mail",
    "~/Library/Messages",
    "~/Library/Mobile Documents",
    "~/Library/CloudStorage",
    "~/Library/Preferences",
    "~/Library/Application Support/MobileSync",
];

#[derive(Debug, PartialEq)]
pub enum Refusal {
    NotAbsolute,
    ParentRef,
    OutsideHome,
    TooShallow,
    Protected(String),
    Symlink(PathBuf),
    Missing,
}

impl Refusal {
    pub fn message(&self) -> String {
        use crate::i18n::{t, tf};
        match self {
            Refusal::NotAbsolute | Refusal::ParentRef => t("guard.bad_path"),
            Refusal::OutsideHome => t("guard.outside_home"),
            Refusal::TooShallow => t("guard.too_shallow"),
            Refusal::Protected(p) => tf("guard.protected", &[("path", p)]),
            Refusal::Symlink(p) => tf("guard.symlink", &[("path", &p.display())]),
            Refusal::Missing => t("guard.missing"),
        }
    }
}

pub fn check(path: &Path) -> Result<(), Refusal> {
    check_in(path, &home())
}

pub fn check_in(path: &Path, home: &Path) -> Result<(), Refusal> {
    if !path.is_absolute() {
        return Err(Refusal::NotAbsolute);
    }
    if path
        .components()
        .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(Refusal::ParentRef);
    }
    let rel = path.strip_prefix(home).map_err(|_| Refusal::OutsideHome)?;
    if rel.components().count() < 2 {
        return Err(Refusal::TooShallow);
    }
    for p in PROTECTED_TREES {
        let prot = match p.strip_prefix("~/") {
            Some(r) => home.join(r),
            None => expand(p),
        };
        if path.starts_with(&prot) || prot.starts_with(path) {
            return Err(Refusal::Protected(p.to_string()));
        }
    }
    // Every component below home, including the target itself, must be a real entry.
    let mut cur = home.to_path_buf();
    for c in rel.components() {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(md) if md.file_type().is_symlink() => return Err(Refusal::Symlink(cur)),
            Ok(_) => {}
            Err(_) => return Err(Refusal::Missing),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup() -> (tempfile::TempDir, PathBuf) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().canonicalize().unwrap();
        fs::create_dir_all(home.join("Library/Caches/App")).unwrap();
        fs::create_dir_all(home.join("Documents/x")).unwrap();
        fs::create_dir_all(home.join("Library/Keychains")).unwrap();
        (d, home)
    }

    #[test]
    fn allows_cache_folder() {
        let (_d, h) = setup();
        assert_eq!(check_in(&h.join("Library/Caches/App"), &h), Ok(()));
    }

    #[test]
    fn refuses_home_and_first_level() {
        let (_d, h) = setup();
        assert_eq!(check_in(&h, &h), Err(Refusal::TooShallow));
        assert_eq!(check_in(&h.join("Library"), &h), Err(Refusal::TooShallow));
    }

    #[test]
    fn refuses_protected_and_their_ancestors() {
        let (_d, h) = setup();
        assert!(matches!(
            check_in(&h.join("Documents/x"), &h),
            Err(Refusal::Protected(_))
        ));
        // ~/Library/Keychains lives under ~/Library, which is too shallow anyway;
        // check a deeper ancestor case with Application Support/MobileSync.
        fs::create_dir_all(h.join("Library/Application Support/MobileSync/Backup")).unwrap();
        assert!(matches!(
            check_in(&h.join("Library/Application Support"), &h),
            Err(Refusal::Protected(_))
        ));
        assert!(matches!(
            check_in(&h.join("Library/Keychains"), &h),
            Err(Refusal::Protected(_))
        ));
    }

    #[test]
    fn refuses_outside_home_and_relative() {
        let (_d, h) = setup();
        assert_eq!(
            check_in(Path::new("/usr/local/lib"), &h),
            Err(Refusal::OutsideHome)
        );
        assert_eq!(
            check_in(Path::new("Library/Caches"), &h),
            Err(Refusal::NotAbsolute)
        );
        assert_eq!(
            check_in(&h.join("Library/Caches/../Caches/App"), &h),
            Err(Refusal::ParentRef)
        );
    }

    #[test]
    fn refuses_symlink_anywhere_on_the_way() {
        let (_d, h) = setup();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), h.join("Library/Caches/link")).unwrap();
        fs::create_dir_all(outside.path().join("inner")).unwrap();
        assert!(matches!(
            check_in(&h.join("Library/Caches/link"), &h),
            Err(Refusal::Symlink(_))
        ));
        assert!(matches!(
            check_in(&h.join("Library/Caches/link/inner"), &h),
            Err(Refusal::Symlink(_))
        ));
    }

    #[test]
    fn refuses_missing() {
        let (_d, h) = setup();
        assert_eq!(
            check_in(&h.join("Library/Caches/nope"), &h),
            Err(Refusal::Missing)
        );
    }
}
