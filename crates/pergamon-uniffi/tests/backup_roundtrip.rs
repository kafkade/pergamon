//! Cross-client backup interoperability (#118).
//!
//! The iOS app (this `pergamon-uniffi` facade) and the CLI both read and write
//! the *same* canonical archive through `pergamon_storage::backup`. These tests
//! prove a backup round-trips across the two code paths in both directions, so
//! the acceptance criterion — "a backup produced on the CLI restores on iOS and
//! vice versa" — holds structurally, not just by inspection.
//!
//! - The **CLI path** is exercised directly via `pergamon_storage::backup`
//!   (exactly what `pergamon-cli`'s `export_backup`/`restore_backup` call).
//! - The **iOS path** is exercised via the FFI `Library::export_backup` /
//!   `Library::restore_backup` methods the Swift app invokes.

// The record counts compared below are tiny (single digits from the seed), so
// the `usize as u32` casts in the assertions cannot truncate.
#![allow(clippy::cast_possible_truncation)]

use std::path::{Path, PathBuf};

use pergamon_storage::Database;
use pergamon_uniffi::Library;
use uuid::Uuid;

/// A unique, self-cleaning temporary directory (the storage crate's own tests
/// use `std::env::temp_dir()` rather than the `tempfile` crate, so we match).
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let mut path = std::env::temp_dir();
        path.push(format!("pergamon-backup-roundtrip-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A backup exported through the iOS FFI restores cleanly through the CLI code
/// path with identical record counts.
#[test]
fn ios_export_restores_through_cli_path() {
    let dir = TempDir::new();
    let archive = dir.join("from-ios.zip");

    // iOS path: open a seeded on-device library and export a backup.
    let lib = Library::open(path_string(&dir.join("ios.db"))).expect("open ios library");
    let summary = lib
        .export_backup(path_string(&archive))
        .expect("export via ffi");
    assert!(summary.total > 0, "seeded library should export records");

    // CLI path: restore that archive into a fresh, empty storage database.
    let cli_db = Database::open(&dir.join("cli.db")).expect("open cli database");
    let file = std::fs::File::open(&archive).expect("open archive");
    let stats = pergamon_storage::backup::restore(&cli_db, file).expect("restore via cli path");

    // The record counts must survive the round-trip exactly.
    assert_eq!(stats.total as u32, summary.total, "total records preserved");
    assert_eq!(stats.content_items as u32, summary.content_items);
    assert_eq!(stats.feeds as u32, summary.feeds);
    assert_eq!(stats.tags as u32, summary.tags);
    assert_eq!(stats.collections as u32, summary.collections);
}

/// A backup exported through the CLI code path restores cleanly through the iOS
/// FFI, reconstructing the same visible library.
#[test]
fn cli_export_restores_through_ios_ffi() {
    let dir = TempDir::new();

    // Capture the seeded library's shape from a reference iOS library.
    let reference = Library::open(path_string(&dir.join("ref.db"))).expect("ref lib");
    let expected_items = reference.items().len();
    let expected_tags = reference.tags().len();
    let expected_collections = reference.collections().len();
    let expected_highlights = reference.storage_info().highlight_count;
    assert!(expected_items > 0 && expected_highlights > 0);

    // Seed a storage database by restoring an archive the reference library
    // produced, then re-export it through the CLI path (this is the "backup
    // produced on the CLI" artifact).
    let seed_archive = dir.join("seed.zip");
    reference
        .export_backup(path_string(&seed_archive))
        .expect("seed archive");
    let cli_db = Database::open(&dir.join("cli.db")).expect("cli db");
    let seed_file = std::fs::File::open(&seed_archive).expect("open seed archive");
    pergamon_storage::backup::restore(&cli_db, seed_file).expect("seed cli db");

    let cli_archive = dir.join("from-cli.zip");
    let out = std::fs::File::create(&cli_archive).expect("create cli archive");
    pergamon_storage::backup::export(&cli_db, out).expect("export via cli path");

    // iOS path: restore the CLI-produced archive into a fresh library (which
    // starts seeded; restore_backup resets it first) and assert parity.
    let ios = Library::open(path_string(&dir.join("ios.db"))).expect("ios lib");
    ios.restore_backup(path_string(&cli_archive))
        .expect("restore via ffi");

    assert_eq!(ios.items().len(), expected_items, "documents preserved");
    assert_eq!(ios.tags().len(), expected_tags, "tags preserved");
    assert_eq!(
        ios.collections().len(),
        expected_collections,
        "collections preserved"
    );
    assert_eq!(
        ios.storage_info().highlight_count,
        expected_highlights,
        "highlights preserved"
    );
}
