/**
 * Insert modal: pick an item and insert a reference at the cursor.
 *
 * Similar to BrowseModal but specifically inserts into the active editor.
 */

import { App, Editor, FuzzySuggestModal } from "obsidian";
import { ManifestItem } from "./types";
import { InsertFormat } from "./settings";

export class InsertModal extends FuzzySuggestModal<ManifestItem> {
  private items: ManifestItem[];
  private editor: Editor;
  private insertFormat: InsertFormat;

  constructor(
    app: App,
    items: ManifestItem[],
    editor: Editor,
    insertFormat: InsertFormat
  ) {
    super(app);
    this.items = items;
    this.editor = editor;
    this.insertFormat = insertFormat;
    this.setPlaceholder("Insert pergamon reference…");
  }

  getItems(): ManifestItem[] {
    return this.items;
  }

  getItemText(item: ManifestItem): string {
    const parts = [item.title];
    if (item.author) {
      parts.push(`by ${item.author}`);
    }
    if (item.highlight_count > 0) {
      parts.push(`(${item.highlight_count} highlights)`);
    }
    return parts.join(" — ");
  }

  onChooseItem(item: ManifestItem): void {
    const reference = formatReference(item, this.insertFormat);
    this.editor.replaceSelection(reference);
  }
}

/** Format an item reference based on the configured insert format. */
export function formatReference(
  item: ManifestItem,
  format: InsertFormat
): string {
  // Extract the note name (filename without .md extension).
  const noteName = item.file_path
    .split("/")
    .pop()
    ?.replace(/\.md$/, "") ?? item.title;

  switch (format) {
    case "wikilink":
      return `[[${noteName}|${item.title}]]`;
    case "markdown":
      return `[${item.title}](${encodeURI(item.file_path)})`;
    case "embed":
      return `![[${noteName}]]`;
    default:
      return `[[${noteName}|${item.title}]]`;
  }
}
