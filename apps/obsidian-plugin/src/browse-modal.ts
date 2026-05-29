/**
 * Browse modal: fuzzy-search through pergamon items.
 *
 * Uses Obsidian's built-in FuzzySuggestModal for instant search.
 */

import { App, FuzzySuggestModal } from "obsidian";
import { ManifestItem } from "./types";
import { InsertFormat } from "./settings";

export class BrowseModal extends FuzzySuggestModal<ManifestItem> {
  private items: ManifestItem[];
  private insertFormat: InsertFormat;
  private onChoose: (item: ManifestItem, format: InsertFormat) => void;

  constructor(
    app: App,
    items: ManifestItem[],
    insertFormat: InsertFormat,
    onChoose: (item: ManifestItem, format: InsertFormat) => void
  ) {
    super(app);
    this.items = items;
    this.insertFormat = insertFormat;
    this.onChoose = onChoose;
    this.setPlaceholder("Search pergamon items…");
  }

  getItems(): ManifestItem[] {
    return this.items;
  }

  getItemText(item: ManifestItem): string {
    const parts = [item.title];
    if (item.author) {
      parts.push(`by ${item.author}`);
    }
    if (item.tags.length > 0) {
      parts.push(`[${item.tags.join(", ")}]`);
    }
    if (item.highlight_count > 0) {
      parts.push(`(${item.highlight_count} highlights)`);
    }
    return parts.join(" — ");
  }

  onChooseItem(item: ManifestItem): void {
    this.onChoose(item, this.insertFormat);
  }
}
