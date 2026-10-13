//! Snapshot files kept next to a notebook: `chatterg.db.backups/`.
//!
//! A snapshot is a complete, consistent copy of the notebook taken with SQLite's
//! `VACUUM INTO`, so it can be opened like any notebook. Names carry everything
//! needed to list them: `auto-20261009-153012-q0037.db`
//! (kind, time taken, number of questions finished).
//!
//! Only `auto` snapshots are ever deleted automatically. `manual` ones (you asked
//! for them) and `safety` ones (taken just before a rollback, rewind or restart)
//! stay until you remove them.

use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Auto,
    Manual,
    Safety,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
            Self::Safety => "safety",
        }
    }

    fn from_word(word: &str) -> Option<Self> {
        match word {
            "auto" => Some(Self::Auto),
            "manual" => Some(Self::Manual),
            "safety" => Some(Self::Safety),
            _ => None,
        }
    }
}

/// How often automatic snapshots are taken and how many are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupPolicy {
    /// A snapshot after every this many finished questions (0 turns them off).
    pub every: usize,
    /// How many automatic snapshots to keep (0 turns them off).
    pub keep: usize,
}

impl Default for BackupPolicy {
    fn default() -> Self {
        Self { every: 25, keep: 10 }
    }
}

impl BackupPolicy {
    pub fn is_on(&self) -> bool {
        self.every > 0 && self.keep > 0
    }
}

/// One snapshot on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub path: PathBuf,
    pub kind: Kind,
    pub taken_at: DateTime<Utc>,
    /// Questions finished when it was taken.
    pub position: usize,
    pub bytes: u64,
}

impl Snapshot {
    pub fn file_name(&self) -> String {
        self.path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

const TIME_FORMAT: &str = "%Y%m%d-%H%M%S";

/// `<notebook>.backups` next to the notebook.
pub fn directory_for(store: &Path) -> PathBuf {
    let mut name = store.as_os_str().to_owned();
    name.push(".backups");
    PathBuf::from(name)
}

pub fn file_name(kind: Kind, taken_at: DateTime<Utc>, position: usize) -> String {
    format!("{}-{}-q{position:04}.db", kind.word(), taken_at.format(TIME_FORMAT))
}

/// Reads a snapshot's kind, time and position back out of its file name.
pub fn parse_name(name: &str) -> Option<(Kind, DateTime<Utc>, usize)> {
    let stem = name.strip_suffix(".db")?;

    // an optional "-2", "-3" ... suffix keeps names unique within one second
    let mut parts = stem.splitn(4, '-');
    let kind = Kind::from_word(parts.next()?)?;
    let date = parts.next()?;
    let rest = parts.next()?;
    let tail = parts.next()?;

    let position = tail.strip_prefix('q')?.split('-').next()?.parse().ok()?;
    let taken_at =
        NaiveDateTime::parse_from_str(&format!("{date}-{rest}"), TIME_FORMAT).ok()?.and_utc();

    Some((kind, taken_at, position))
}

/// All snapshots of this notebook, newest first.
pub fn list(store: &Path) -> Vec<Snapshot> {
    let Ok(entries) = std::fs::read_dir(directory_for(store)) else {
        return Vec::new();
    };

    let mut snapshots: Vec<Snapshot> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let (kind, taken_at, position) = parse_name(&entry.file_name().to_string_lossy())?;
            let bytes = entry.metadata().ok()?.len();

            Some(Snapshot { path: entry.path(), kind, taken_at, position, bytes })
        })
        .collect();

    // newest first; the file name breaks ties between snapshots of the same second
    snapshots.sort_by(|a, b| (b.taken_at, b.file_name()).cmp(&(a.taken_at, a.file_name())));
    snapshots
}

/// A path for a new snapshot that does not exist yet.
pub fn unused_path(store: &Path, kind: Kind, taken_at: DateTime<Utc>, position: usize) -> PathBuf {
    let directory = directory_for(store);
    let name = file_name(kind, taken_at, position);
    let first = directory.join(&name);

    if !first.exists() {
        return first;
    }

    let stem = name.trim_end_matches(".db");
    (2..)
        .map(|n| directory.join(format!("{stem}-{n}.db")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(first)
}

/// Deletes the oldest automatic snapshots beyond `keep`. Manual and safety
/// snapshots are never touched. Returns how many were removed.
pub fn prune(store: &Path, keep: usize) -> usize {
    let automatic: Vec<Snapshot> =
        list(store).into_iter().filter(|snapshot| snapshot.kind == Kind::Auto).collect();

    automatic
        .into_iter()
        .skip(keep)
        .filter(|snapshot| std::fs::remove_file(&snapshot.path).is_ok())
        .count()
}
