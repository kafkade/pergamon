<!-- markdownlint-disable MD024 -->
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Repository scaffolding: GitHub templates, CI workflow, copilot instructions, contribution guide, and licensing (Apache-2.0)
- Architecture Decision Records (`docs/adr/ADR-001` through `ADR-010`)
- Product roadmap (`docs/roadmap.md`)
- Cargo workspace with five crates: `pergamon-core`, `pergamon-storage`, `pergamon-feed`, `pergamon-extract`, `pergamon-cli`
- CLI binary with `--info` flag (`pergamon --info`)
- Workspace-wide lint configuration (forbid unsafe, deny unwrap/expect/panic, clippy pedantic + nursery)
- Rust CI pipeline: check, test (Linux/macOS/Windows), clippy, fmt
