//! Regression test for issue #188, acceptance criterion 1: a normal launch /
//! operation must **not** force any key ceremony. Running an everyday command
//! (here: creating a collection, and a read-only feed list) must create **no**
//! device keys and **no** Account Root Key in a fresh key store.
//!
//! The key store is only ever meant to be touched by the `sync-*` / `device-key`
//! subcommands. This test locks that invariant in end-to-end by running the real
//! binary against an isolated `HOME` + database and asserting no key material
//! was written.
//!
//! We set `PERGAMON_KEY_PASSPHRASE` so that *if* a normal command ever opened
//! the key store it would select the encrypted-file backend, whose first save
//! materializes `sync-keys.json` under the config dir — making any accidental
//! key/ARK creation observable as a file. Because normal commands never open the
//! store, no such file appears.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// Path to the compiled `pergamon` binary under test.
const BIN: &str = env!("CARGO_BIN_EXE_pergamon");

/// Create a unique, isolated temp directory to act as `HOME` and data dir.
fn unique_tmp_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("pergamon-it-{}-{}", std::process::id(), nanos));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `pergamon <args>` in the isolated environment rooted at `home`.
fn run(home: &Path, db: &Path, args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .env("HOME", home)
        // Force the file-backed key store *if* the store is ever opened, so any
        // key/ARK save would leave an observable file.
        .env("PERGAMON_KEY_PASSPHRASE", "regression-test-passphrase")
        .env("PERGAMON_DB", db)
        // Keep config/keys under the isolated HOME on Linux too.
        .env("XDG_CONFIG_HOME", home.join(".config"))
        // Don't inherit a stray data dir from the outer environment.
        .env_remove("PERGAMON_DATA_DIR")
        .output()
        .expect("failed to run pergamon binary")
}

/// Recursively search `dir` for any file named `name`.
fn contains_file_named(dir: &Path, name: &str) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if contains_file_named(&path, name) {
                return true;
            }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return true;
        }
    }
    false
}

#[test]
fn normal_operations_create_no_key_material() {
    let home = unique_tmp_dir();
    let db = home.join("pergamon.db");

    // A normal write operation: create a collection.
    let create = run(&home, &db, &["collection", "create", "Reading"]);
    assert!(
        create.status.success(),
        "`collection create` should succeed; stderr: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    // A normal read operation: list feeds.
    let list = run(&home, &db, &["feed", "list"]);
    assert!(
        list.status.success(),
        "`feed list` should succeed; stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );

    // No encrypted key file may have been written anywhere under HOME: normal
    // commands must never save device keys or an ARK.
    assert!(
        !contains_file_named(&home, "sync-keys.json"),
        "a normal operation created a key store file — key ceremony must not be forced"
    );

    // Positive probe: asking the key store directly must report no device keys,
    // confirming neither device keys nor an ARK were created.
    let show = run(&home, &db, &["device-key", "show", "--account", "default"]);
    assert!(
        show.status.success(),
        "`device-key show` should succeed; stderr: {}",
        String::from_utf8_lossy(&show.stderr)
    );
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert!(
        stdout.contains("No device keys stored"),
        "expected no device keys after normal operations, got: {stdout}"
    );

    // Best-effort cleanup.
    let _ = fs::remove_dir_all(&home);
}
