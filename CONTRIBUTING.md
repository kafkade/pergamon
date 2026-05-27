# Contributing to pergamon

Thank you for your interest in contributing to pergamon! This document covers how to
build the project, our development workflow, and contribution requirements.

## Code of Conduct

This project follows the [Contributor Covenant Code of Conduct](CODE_OF_CONDUCT.md). Please be respectful and constructive in all interactions.

## Prerequisites

- **Rust stable** — install via [rustup](https://rustup.rs/).
- **cargo-deny** (optional, runs in CI) — install with `cargo install cargo-deny` or download a prebuilt binary from the [cargo-deny releases](https://github.com/EmbarkStudios/cargo-deny/releases) page.

## Building from Source

```sh
# Clone the repository
git clone https://github.com/kafkade/pergamon.git
cd pergamon

# Build all crates
cargo build --workspace

# Build the CLI
cargo build -p pergamon-cli
```

## Running Tests

```sh
# Run all tests
cargo test --workspace

# Run tests for a specific crate
cargo test -p pergamon-core

# Run a single test by name
cargo test -p pergamon-core test_name

# Run tests for a specific module
cargo test -p pergamon-core feeds::
```

## Code Quality

All code must pass these checks before merging:

```sh
# Formatting (rustfmt)
cargo fmt --check
# Fix formatting issues:
cargo fmt

# Linting (clippy)
cargo clippy --workspace --all-targets -- -D warnings

# Run the full test suite
cargo test --workspace

# All checks run in CI on every pull request
```

## Development Workflow

1. **Fork and clone** the repository.
2. **Create a feature branch** from `main`:

   ```sh
   git checkout -b feat/my-feature
   ```

3. **Make your changes** and ensure all checks pass.
4. **Sign off your commits** (DCO requirement — see below):

   ```sh
   git commit -s -m "feat: add my feature"
   ```

5. **Open a pull request** against `main`.

### Commit Messages

We follow [Conventional Commits](https://www.conventionalcommits.org/):

- `feat:` — new feature
- `fix:` — bug fix
- `docs:` — documentation changes
- `test:` — adding or updating tests
- `refactor:` — code restructuring without behavior change
- `chore:` — maintenance tasks (CI, dependencies, etc.)

For multi-component changes, include the component:
`feat(feed): add OPML import parser`

### Pull Request Checklist

- [ ] Tests pass (`cargo test --workspace`)
- [ ] Clippy passes (`cargo clippy --workspace -- -D warnings`)
- [ ] Formatting passes (`cargo fmt --check`)
- [ ] Commits are signed off (DCO)
- [ ] PR description follows the template

## Architecture Guidelines

### pergamon-core Must Have Zero I/O

The core library (`crates/pergamon-core/`) must not depend on networking, file system
access, or platform-specific APIs. All I/O happens in platform-specific code
(CLI, iOS, web). This keeps the core testable and compilable to WASM.

### Unified Content Model

All content types — feed items, articles, bookmarks, highlights, PDFs, newsletters —
share a single `document` entity with a `content_type` discriminator. Content type
is a filter, not a silo. Changes to the domain model must consider all content
types — never introduce type-specific silos.

### Error Handling

- Use `thiserror` for library errors in core crates.
- Use `anyhow` only in binary crates (`pergamon-cli`).

## Developer Certificate of Origin (DCO)

All contributions to this project must be signed off under the
[Developer Certificate of Origin](DCO) (DCO). By signing off your commits, you
certify that you wrote the code or have the right to submit it under the
project's license.

Add the sign-off to your commits with `git commit -s` or manually:

```text
Signed-off-by: Your Name <your.email@example.com>
```

This is a lightweight alternative to a CLA (Contributor License Agreement),
used by projects like the Linux kernel and many CNCF projects.

## License

All code is licensed under [Apache-2.0](LICENSE).

A future sync server (`crates/pergamon-server/`) will be licensed under AGPL-3.0.

By contributing, you agree that your contributions will be licensed under the
same license as the component you are contributing to.
