# ADR-009: CLI/TUI Design and Command Structure

**Status**: Accepted  
**Date**: 2026-05-22  
**Deciders**: kafkade

## Context

pergamon is explicitly CLI-first and TUI-first. That is not a fallback interface; it is the primary product surface. The command design therefore needs to carry a large amount of product functionality: feed subscription, bookmarking, search, review sessions, import/export, tagging, collections, configuration, and diagnostics. The system must feel coherent for shell users while still supporting richer reading and review workflows in a terminal UI.

The toolchain should be idiomatic Rust and stable over time. `clap` v4 is the obvious choice for command parsing because it supports derive macros, rich help output, validation, and shell completion generation. For interactive terminal screens, `ratatui` plus `crossterm` provides a modern ecosystem path without introducing a GUI dependency. The output model must also respect scripting use cases. pergamon is not just for manual use; it should interoperate with shell pipelines, automation, and exports.

Because the application replaces multiple tools, command naming matters. Commands must be intuitive and map to user intent rather than internal architecture. The result should be broad enough for ingestion and review workflows but still predictable for a solo-maintained open source tool. Accessibility and terminal etiquette also matter, including support for `NO_COLOR`.

## Decision

pergamon will use:
- `clap` v4 with derive macros for CLI parsing and help
- `ratatui` + `crossterm` for interactive TUI screens

The command hierarchy will be:

- `pergamon feed add/list/remove/refresh/discover`
- `pergamon read`
- `pergamon save <url>`
- `pergamon search <query>`
- `pergamon review`
- `pergamon import inoreader/raindrop/readwise/pocket/kindle`
- `pergamon export opml/json/csv/markdown/obsidian/backup`
- `pergamon tag add/list/remove`
- `pergamon collection create/list/move`
- `pergamon config`
- `pergamon doctor`

Machine-readable and tabular output will be supported through a shared `--format` flag with `table` as the default and `json` and `csv` as structured alternatives. `NO_COLOR` will be respected. Shell completions will be auto-generated using `clap_complete`.

Interactive reading and review sessions will use TUI flows, while list/search/export operations remain scriptable CLI commands.

## Consequences

### Positive
- Matches pergamon’s CLI/TUI-first identity.
- Gives both human-friendly and automation-friendly interfaces.
- Uses mature Rust libraries with strong ecosystem support.
- Establishes a discoverable command hierarchy from the start.
- Supports terminal conventions such as `NO_COLOR` and shell completions.

### Negative
- Maintaining both command-style and TUI-style interactions adds UX complexity.
- Command hierarchy breadth increases documentation burden.
- Some advanced workflows may require careful coordination between flags and TUI states.
- Terminal rendering quirks can vary across platforms and shells.

## Rejected Alternatives

- **Build only a TUI and skip a rich CLI**: rejected because scripting, automation, and composability are core to pergamon’s design.
- **Build only subcommands and avoid TUI entirely**: rejected because reading and review sessions benefit from interactive terminal experiences.
- **Use manual argument parsing**: rejected because `clap` v4 already solves parsing, help, validation, and completion generation well.
- **Adopt a GUI-first application model**: rejected because pergamon is intentionally local-first, shell-oriented, and optimized for terminal workflows.
