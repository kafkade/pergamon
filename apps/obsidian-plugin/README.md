# Pergamon — Obsidian Plugin

Browse, search, and insert references to your [pergamon](https://github.com/kafkade/pergamon) highlights and bookmarks directly inside Obsidian.

## How It Works

1. **Export from pergamon** — Run `pergamon export obsidian --vault /path/to/vault` to export highlights and bookmarks as Markdown notes with frontmatter.
2. **Browse in Obsidian** — Use the command palette (`Ctrl/Cmd+P`) → *Browse pergamon items* to fuzzy-search your library.
3. **Insert references** — Use *Insert pergamon reference* to drop a wikilink, markdown link, or embed at your cursor.

The plugin is a **read-only consumer** — it never modifies the exported files. Pergamon owns the export folder; you own everything else in your vault.

## Data Flow

```text
pergamon CLI → export → vault/{folder}/
                              ├── Highlights/
                              │   └── {source}--{uuid}.md
                              ├── Bookmarks/
                              │   └── {bookmark}--{uuid}.md
                              └── .pergamon/
                                  └── manifest.json  ← plugin reads this
```

The plugin reads `manifest.json` via Obsidian's Vault API (no Node `fs`, no network calls) to build its search index.

## Commands

| Command | Description |
|---------|-------------|
| **Browse pergamon items** | Fuzzy-search all exported items and open them |
| **Insert pergamon reference** | Search and insert a wikilink/link/embed at cursor |
| **Reload pergamon manifest** | Re-read the manifest after a fresh export |
| **Show pergamon stats** | Display item counts and last export time |

## Settings

| Setting | Default | Description |
|---------|---------|-------------|
| Pergamon folder | `Pergamon` | Folder where pergamon exports notes. Must match `--folder` flag. |
| Insert format | Wikilink | How references are inserted: wikilink, markdown link, or embed |
| Show ribbon icon | On | Toggle the Pergamon icon in the left ribbon |

## Installation

### From Obsidian Community Plugins (coming soon)

1. Open Settings → Community Plugins → Browse
2. Search for "Pergamon"
3. Install and enable

### Manual Installation

1. Build the plugin: `npm install && npm run build`
2. Copy `main.js`, `manifest.json`, and `styles.css` to `{vault}/.obsidian/plugins/pergamon/`
3. Enable "Pergamon" in Settings → Community Plugins

## Development

```sh
cd apps/obsidian-plugin
npm install
npm run dev    # watch mode with hot reload
npm run build  # production build
npm run lint   # type-check
```

## Requirements

- Obsidian ≥ 1.0.0
- pergamon CLI (`pergamon export obsidian`)

## License

Apache-2.0 — same as the main pergamon project.
