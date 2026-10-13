//! Backups: file names, automatic snapshots, pruning, and notebooks that are damaged.

use std::path::Path;

use chatterg::{
    domain::Conversation,
    storage::{
        StorageError, Store,
        backups::{self, BackupPolicy, Kind},
        sqlite::SqliteStore,
    },
};
use chrono::{TimeZone, Utc};

fn at(position: usize) -> Conversation {
    Conversation { position, messages_sent: position as u64, ..Conversation::default() }
}

fn when(second: u32) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 9, 15, 30, second).unwrap()
}

fn names_in(store: &Path) -> Vec<String> {
    backups::list(store).iter().map(|snapshot| snapshot.file_name()).collect()
}

fn touch(store: &Path, kind: Kind, second: u32, position: usize) {
    let path = backups::unused_path(store, kind, when(second), position);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"x").unwrap();
}

// ---- names -----------------------------------------------------------------------

#[test]
fn a_name_carries_kind_time_and_position_and_reads_back() {
    let name = backups::file_name(Kind::Auto, when(12), 37);

    assert_eq!(name, "auto-20261009-153012-q0037.db");
    assert_eq!(backups::parse_name(&name), Some((Kind::Auto, when(12), 37)));
    assert_eq!(
        backups::parse_name("safety-20261009-153012-q0001.db"),
        Some((Kind::Safety, when(12), 1))
    );
    assert_eq!(
        backups::parse_name("manual-20261009-153012-q12345.db"),
        Some((Kind::Manual, when(12), 12345))
    );
}

#[test]
fn a_name_made_unique_with_a_suffix_still_reads_back() {
    assert_eq!(
        backups::parse_name("auto-20261009-153012-q0037-2.db"),
        Some((Kind::Auto, when(12), 37))
    );
}

#[test]
fn names_that_are_not_snapshots_are_ignored() {
    for name in [
        "random.db",
        "auto-2026-q1.db",
        "other-20261009-153012-q0001.db",
        "auto-20261009-153012-q0001.txt",
        "auto-20261009-153012-qabc.db",
        "auto-20261340-253012-q0001.db",
        "",
    ] {
        assert_eq!(backups::parse_name(name), None, "{name:?}");
    }
}

#[test]
fn the_backup_folder_sits_next_to_the_notebook() {
    assert_eq!(
        backups::directory_for(Path::new("work/chatterg.db")),
        Path::new("work/chatterg.db.backups")
    );
}

// ---- listing and pruning --------------------------------------------------------------

#[test]
fn snapshots_are_listed_newest_first_and_junk_is_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("c.db");

    touch(&store, Kind::Auto, 1, 2);
    touch(&store, Kind::Manual, 3, 3);
    touch(&store, Kind::Auto, 2, 4);
    std::fs::write(backups::directory_for(&store).join("notes.txt"), "junk").unwrap();

    assert_eq!(
        names_in(&store),
        [
            "manual-20261009-153003-q0003.db",
            "auto-20261009-153002-q0004.db",
            "auto-20261009-153001-q0002.db"
        ]
    );
}

#[test]
fn listing_a_notebook_without_backups_is_empty_not_an_error() {
    let dir = tempfile::tempdir().unwrap();

    assert!(backups::list(&dir.path().join("c.db")).is_empty());
}

#[test]
fn two_snapshots_in_the_same_second_get_different_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("c.db");

    touch(&store, Kind::Auto, 5, 7);
    touch(&store, Kind::Auto, 5, 7);
    touch(&store, Kind::Auto, 5, 7);

    assert_eq!(backups::list(&store).len(), 3);
}

#[test]
fn pruning_removes_only_old_automatic_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("c.db");

    for second in 1..=5 {
        touch(&store, Kind::Auto, second, second as usize);
    }
    touch(&store, Kind::Manual, 1, 1);
    touch(&store, Kind::Safety, 1, 1);
    touch(&store, Kind::Safety, 2, 2);

    let removed = backups::prune(&store, 2);

    assert_eq!(removed, 3);
    let left = backups::list(&store);
    assert_eq!(left.iter().filter(|s| s.kind == Kind::Auto).count(), 2);
    assert_eq!(
        left.iter().filter(|s| s.kind == Kind::Manual).count(),
        1,
        "manual backups are never pruned"
    );
    assert_eq!(
        left.iter().filter(|s| s.kind == Kind::Safety).count(),
        2,
        "safety copies are never pruned"
    );
    // it is the newest automatic ones that stay
    let automatic: Vec<_> =
        left.iter().filter(|s| s.kind == Kind::Auto).map(|s| s.position).collect();
    assert_eq!(automatic, [5, 4]);
}

// ---- automatic snapshots ----------------------------------------------------------------

fn store_with(dir: &Path, every: usize, keep: usize) -> (SqliteStore, std::path::PathBuf) {
    let path = dir.join("c.db");
    let store = SqliteStore::open(&path).unwrap().with_backups(&path, BackupPolicy { every, keep });

    (store, path)
}

#[tokio::test]
async fn a_snapshot_is_taken_every_n_finished_questions() {
    let dir = tempfile::tempdir().unwrap();
    let (store, path) = store_with(dir.path(), 2, 10);

    for position in 1..=6 {
        store.save(&at(position)).await.unwrap();
    }

    let mut positions: Vec<_> = backups::list(&path).iter().map(|s| s.position).collect();
    positions.sort_unstable();
    assert_eq!(positions, [2, 4, 6]);
    assert!(backups::list(&path).iter().all(|s| s.kind == Kind::Auto));

    // each one is a complete notebook of its own
    for snapshot in backups::list(&path) {
        let copy = SqliteStore::open_read_only(&snapshot.path).unwrap();
        assert_eq!(copy.load().await.unwrap().unwrap().position, snapshot.position);
    }
}

#[tokio::test]
async fn retrying_the_same_question_does_not_make_more_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let (store, path) = store_with(dir.path(), 2, 10);

    for _ in 0..5 {
        store.save(&at(2)).await.unwrap(); // saved after every attempt, position unchanged
    }

    assert_eq!(backups::list(&path).len(), 1);
}

#[tokio::test]
async fn only_the_newest_snapshots_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let (store, path) = store_with(dir.path(), 2, 2);

    for position in 1..=8 {
        store.save(&at(position)).await.unwrap();
    }

    let mut positions: Vec<_> = backups::list(&path).iter().map(|s| s.position).collect();
    positions.sort_unstable();
    assert_eq!(positions, [6, 8]);
}

#[tokio::test]
async fn nothing_is_snapshotted_at_the_very_start_or_when_switched_off() {
    let dir = tempfile::tempdir().unwrap();
    let (store, path) = store_with(dir.path(), 2, 10);
    store.save(&at(0)).await.unwrap();
    assert!(!backups::directory_for(&path).exists());

    for (every, keep) in [(0, 10), (2, 0)] {
        let sub = tempfile::tempdir().unwrap();
        let (store, path) = store_with(sub.path(), every, keep);
        for position in 1..=6 {
            store.save(&at(position)).await.unwrap();
        }
        assert!(!backups::directory_for(&path).exists(), "every={every} keep={keep}");
    }
}

#[tokio::test]
async fn a_backup_that_cannot_be_written_is_a_warning_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let (store, path) = store_with(dir.path(), 2, 10);

    // a file where the backup folder should be
    std::fs::write(backups::directory_for(&path), "in the way").unwrap();

    store.save(&at(2)).await.expect("the save itself must still succeed");

    let warnings = store.take_warnings();
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("could not write a backup"), "{}", warnings[0]);
    assert!(store.take_warnings().is_empty(), "a warning is reported once");

    store.save(&at(3)).await.unwrap();
    assert_eq!(store.load().await.unwrap().unwrap().position, 3);
}

#[tokio::test]
async fn a_read_only_view_can_take_a_snapshot_while_a_run_holds_the_notebook() {
    let dir = tempfile::tempdir().unwrap();
    let (writer, path) = store_with(dir.path(), 0, 0);
    writer.save(&at(4)).await.unwrap();

    let reader = SqliteStore::open_read_only(&path).unwrap();
    let copy = dir.path().join("copy.db");
    reader.snapshot_to(&copy).unwrap();

    assert_eq!(
        SqliteStore::open_read_only(&copy).unwrap().load().await.unwrap().unwrap().position,
        4
    );
}

// ---- damaged notebooks ---------------------------------------------------------------------

#[test]
fn a_file_that_is_not_a_database_is_reported_as_damaged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");
    std::fs::write(
        &path,
        "this is definitely not sqlite, but it is longer than a header, yes it is, really",
    )
    .unwrap();

    let error = SqliteStore::open(&path).err().unwrap();

    assert!(matches!(&error, StorageError::Damaged { .. }), "{error:?}");
    assert!(error.to_string().contains("is damaged"));
    assert!(matches!(SqliteStore::open_read_only(&path), Err(StorageError::Damaged { .. })));
}

#[tokio::test]
async fn a_notebook_with_broken_pages_is_reported_as_damaged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("c.db");

    {
        let store = SqliteStore::open(&path).unwrap();
        store.save(&at(3)).await.unwrap();
    }
    // wreck the first page's own header (right after the 100-byte file header), as a
    // bad disk might. Damage to unused space would rightly go unnoticed.
    let mut bytes = std::fs::read(&path).unwrap();
    for byte in &mut bytes[100..140] {
        *byte = 0xFF;
    }
    std::fs::write(&path, bytes).unwrap();
    let _ = std::fs::remove_file(dir.path().join("c.db-wal"));
    let _ = std::fs::remove_file(dir.path().join("c.db-shm"));

    let result = SqliteStore::open(&path);

    assert!(matches!(result, Err(StorageError::Damaged { .. })), "{:?}", result.err());
}

#[test]
fn a_missing_notebook_is_not_called_damaged() {
    let dir = tempfile::tempdir().unwrap();

    assert!(matches!(
        SqliteStore::open_read_only(dir.path().join("nope.db")),
        Err(StorageError::Open { .. })
    ));
}
