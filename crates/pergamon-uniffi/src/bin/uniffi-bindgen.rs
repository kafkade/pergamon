//! `uniffi-bindgen` CLI, vendored so the generator version always matches the
//! `uniffi` runtime version compiled into the library.
//!
//! Usage (see `scripts/build-ios.sh`):
//!   cargo run --bin uniffi-bindgen -- generate --library <dylib> \
//!       --language swift --out-dir <dir>

fn main() {
    uniffi::uniffi_bindgen_main();
}
