/**
 * Data bridge: reads the pergamon manifest from the vault.
 *
 * The manifest is written by `pergamon export obsidian --vault <path>`
 * and lives at `{folder}/.pergamon/manifest.json`.
 */

import { App, normalizePath } from "obsidian";
import { PergamonManifest, ManifestItem } from "./types";

export class DataBridge {
  private app: App;
  private folderName: string;
  private manifest: PergamonManifest | null = null;

  constructor(app: App, folderName: string) {
    this.app = app;
    this.folderName = folderName;
  }

  /** Update the folder name when settings change. */
  setFolderName(folderName: string): void {
    this.folderName = folderName;
    this.manifest = null;
  }

  /** Path to the manifest file within the vault. */
  private manifestPath(): string {
    return normalizePath(`${this.folderName}/.pergamon/manifest.json`);
  }

  /** Load or reload the manifest from the vault. */
  async loadManifest(): Promise<PergamonManifest | null> {
    const path = this.manifestPath();
    const file = this.app.vault.getAbstractFileByPath(path);

    if (!file) {
      this.manifest = null;
      return null;
    }

    try {
      const content = await this.app.vault.adapter.read(path);
      this.manifest = JSON.parse(content) as PergamonManifest;

      if (this.manifest.schema_version !== 1) {
        console.warn(
          `Pergamon: unsupported manifest schema version ${this.manifest.schema_version}`
        );
      }

      return this.manifest;
    } catch (e) {
      console.error("Pergamon: failed to read manifest", e);
      this.manifest = null;
      return null;
    }
  }

  /** Get the currently loaded manifest (may be null). */
  getManifest(): PergamonManifest | null {
    return this.manifest;
  }

  /** Get all items from the manifest. */
  getItems(): ManifestItem[] {
    return this.manifest?.items ?? [];
  }

  /** Search items by title (case-insensitive substring match). */
  searchItems(query: string): ManifestItem[] {
    const lower = query.toLowerCase();
    return this.getItems().filter(
      (item) =>
        item.title.toLowerCase().includes(lower) ||
        (item.author?.toLowerCase().includes(lower) ?? false) ||
        item.tags.some((t) => t.toLowerCase().includes(lower))
    );
  }

  /** Get the last export timestamp, formatted for display. */
  getLastExportTime(): string | null {
    return this.manifest?.exported_at ?? null;
  }

  /** Get total item count. */
  getItemCount(): number {
    return this.manifest?.item_count ?? 0;
  }
}
