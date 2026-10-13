//! Going back: a backup on demand, restoring an earlier snapshot, and rewinding a
//! run to an earlier question.
//!
//! Every operation that changes a notebook first takes a `safety` snapshot of what
//! is there, so none of them can lose work, and each one can itself be undone.

use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;

use crate::storage::{
    StorageError, Store,
    backups::{self, Kind, Snapshot},
    sqlite::SqliteStore,
};

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error("there is no run in {0} to back up")]
    NothingToBackUp(String),

    #[error("there are no backups for this notebook yet (looked in {})", directory.display())]
    NoBackups { directory: PathBuf },

    #[error("there is no backup number {number}; `chatterg backups` shows {count}")]
    NoSuchBackup { number: usize, count: usize },

    #[error("the backup {} holds no run", path.display())]
    EmptyBackup { path: PathBuf },

    #[error("there is nothing to rewind: no question has been finished in {0}")]
    NothingToRewind(String),

    #[error(
        "cannot rewind to question {requested}: only questions 1 to {position} have been \
         finished, so the run is already at question {next}"
    )]
    CannotRewind { requested: usize, position: usize, next: usize },

    #[error("cannot move the damaged notebook {} out of the way: {source}", path.display())]
    MoveAside {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// What a rollback did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RolledBack {
    pub restored_from: Snapshot,
    /// Questions finished in the restored run.
    pub position: usize,
    /// Questions in the restored run (0 when the backup does not say).
    pub total: usize,
    /// A copy of what the notebook held just before (nothing if it was empty).
    pub safety: Option<PathBuf>,
    /// Where a damaged notebook was moved to.
    pub moved_aside: Option<PathBuf>,
}

/// What a rewind did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rewound {
    pub from_position: usize,
    pub to_question: usize,
    /// Answers that will be asked again.
    pub dropped: usize,
    pub safety: PathBuf,
}

/// Takes a `manual` snapshot of the notebook. Safe during a run: it only reads.
pub async fn backup_now(store_path: &Path) -> Result<Snapshot, RecoveryError> {
    let store = SqliteStore::open_read_only(store_path)?;

    let conversation = store
        .load()
        .await?
        .ok_or_else(|| RecoveryError::NothingToBackUp(store_path.display().to_string()))?;

    let taken_at = Utc::now();
    let path = backups::unused_path(store_path, Kind::Manual, taken_at, conversation.position);
    store.snapshot_to(&path)?;

    let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);

    Ok(Snapshot { path, kind: Kind::Manual, taken_at, position: conversation.position, bytes })
}

/// Moves a damaged notebook (and its write-ahead files) aside, returning where it went.
fn move_aside(store_path: &Path) -> Result<PathBuf, RecoveryError> {
    let stamp = Utc::now().format("%Y%m%d-%H%M%S");
    let mut aside = store_path.as_os_str().to_owned();
    aside.push(format!(".damaged-{stamp}"));
    let aside = PathBuf::from(aside);

    std::fs::rename(store_path, &aside)
        .map_err(|source| RecoveryError::MoveAside { path: store_path.to_owned(), source })?;

    for extra in ["-wal", "-shm"] {
        let mut from = store_path.as_os_str().to_owned();
        from.push(extra);
        let mut to = aside.as_os_str().to_owned();
        to.push(extra);

        if Path::new(&from).exists() {
            let _ = std::fs::rename(from, to);
        }
    }

    Ok(aside)
}

/// A safety snapshot of whatever the notebook holds right now, if anything.
fn safety_copy(
    store: &SqliteStore,
    store_path: &Path,
    position: usize,
) -> Result<Option<PathBuf>, StorageError> {
    if !store.has_conversation()? {
        return Ok(None);
    }

    let path = backups::unused_path(store_path, Kind::Safety, Utc::now(), position);
    store.snapshot_to(&path)?;

    Ok(Some(path))
}

/// Restores a snapshot into the working notebook.
///
/// `which` is the number `chatterg backups` shows (1 = newest). Without it, the
/// newest snapshot that is not a safety copy is used. A damaged notebook is moved
/// aside rather than deleted. Fails (touching nothing) while another chatterg is
/// using the notebook.
pub async fn rollback(
    store_path: &Path,
    which: Option<usize>,
) -> Result<RolledBack, RecoveryError> {
    let snapshots = backups::list(store_path);

    if snapshots.is_empty() {
        return Err(RecoveryError::NoBackups { directory: backups::directory_for(store_path) });
    }

    let chosen = match which {
        Some(number) => number
            .checked_sub(1)
            .and_then(|index| snapshots.get(index))
            .ok_or(RecoveryError::NoSuchBackup { number, count: snapshots.len() })?,
        None => {
            snapshots.iter().find(|snapshot| snapshot.kind != Kind::Safety).unwrap_or(&snapshots[0])
        }
    };

    // read the snapshot before anything is touched
    let conversation = SqliteStore::open_read_only(&chosen.path)?
        .load()
        .await?
        .ok_or_else(|| RecoveryError::EmptyBackup { path: chosen.path.clone() })?;

    let (store, moved_aside) = match SqliteStore::open(store_path) {
        Ok(store) => (store, None),
        Err(StorageError::Damaged { .. }) => {
            let aside = move_aside(store_path)?;
            (SqliteStore::open(store_path)?, Some(aside))
        }
        Err(error) => return Err(error.into()),
    };

    let current_position = store.load().await.ok().flatten().map_or(0, |current| current.position);
    let safety = safety_copy(&store, store_path, current_position)?;

    store.save(&conversation).await?;

    Ok(RolledBack {
        restored_from: chosen.clone(),
        position: conversation.position,
        total: conversation.questionnaire_snapshot.len(),
        safety,
        moved_aside,
    })
}

/// Cuts the run back so question `to_question` (1-based, as `chatterg list` numbers
/// them) and everything after it is asked again.
pub async fn rewind(store_path: &Path, to_question: usize) -> Result<Rewound, RecoveryError> {
    let store = SqliteStore::open(store_path)?;

    let shown = store_path.display().to_string();
    let mut conversation =
        store.load().await?.ok_or_else(|| RecoveryError::NothingToRewind(shown.clone()))?;

    let position = conversation.position;
    if position == 0 {
        return Err(RecoveryError::NothingToRewind(shown));
    }

    if to_question == 0 || to_question > position {
        return Err(RecoveryError::CannotRewind {
            requested: to_question,
            position,
            next: position + 1,
        });
    }

    let safety = safety_copy(&store, store_path, position)?
        .ok_or_else(|| RecoveryError::NothingToRewind(store_path.display().to_string()))?;

    let dropped = conversation.answers.len().saturating_sub(to_question - 1);
    conversation.rewind_to(to_question - 1);
    store.save(&conversation).await?;

    Ok(Rewound { from_position: position, to_question, dropped, safety })
}
