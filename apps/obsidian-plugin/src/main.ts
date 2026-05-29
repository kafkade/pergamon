/**
 * Pergamon — Obsidian community plugin.
 *
 * Reads the manifest and Markdown notes exported by `pergamon export obsidian`
 * to let users browse, search, and insert references to their pergamon library.
 *
 * Data flow: pergamon CLI → export to vault folder → plugin reads files via Vault API.
 * The plugin never modifies the export folder; it is a read-only consumer.
 */

import { Notice, Plugin } from "obsidian";
import { DataBridge } from "./data-bridge";
import { BrowseModal } from "./browse-modal";
import { InsertModal } from "./insert-modal";
import {
  PergamonSettings,
  PergamonSettingTab,
  DEFAULT_SETTINGS,
} from "./settings";

export default class PergamonPlugin extends Plugin {
  settings: PergamonSettings = DEFAULT_SETTINGS;
  dataBridge: DataBridge = null as unknown as DataBridge;
  private ribbonIconEl: HTMLElement | null = null;

  async onload(): Promise<void> {
    await this.loadSettings();

    this.dataBridge = new DataBridge(this.app, this.settings.folderName);

    this.addSettingTab(new PergamonSettingTab(this.app, this));

    this.updateRibbonIcon();

    // Command: Browse pergamon items
    this.addCommand({
      id: "browse-items",
      name: "Browse pergamon items",
      callback: async () => {
        await this.ensureManifest();
        const items = this.dataBridge.getItems();

        if (items.length === 0) {
          new Notice("No pergamon items found. Run `pergamon export obsidian` first.");
          return;
        }

        new BrowseModal(
          this.app,
          items,
          this.settings.insertFormat,
          (item) => {
            const file = this.app.vault.getAbstractFileByPath(item.file_path);
            if (file) {
              this.app.workspace.openLinkText(item.file_path, "", false);
            } else {
              new Notice(`File not found: ${item.file_path}`);
            }
          }
        ).open();
      },
    });

    // Command: Insert pergamon reference at cursor
    this.addCommand({
      id: "insert-reference",
      name: "Insert pergamon reference",
      editorCallback: async (editor) => {
        await this.ensureManifest();
        const items = this.dataBridge.getItems();

        if (items.length === 0) {
          new Notice("No pergamon items found. Run `pergamon export obsidian` first.");
          return;
        }

        new InsertModal(
          this.app,
          items,
          editor,
          this.settings.insertFormat
        ).open();
      },
    });

    // Command: Reload manifest
    this.addCommand({
      id: "reload-manifest",
      name: "Reload pergamon manifest",
      callback: async () => {
        const manifest = await this.dataBridge.loadManifest();
        if (manifest) {
          new Notice(
            `Pergamon: loaded ${manifest.item_count} items ` +
              `(exported ${manifest.exported_at})`
          );
        } else {
          new Notice(
            "Pergamon: no manifest found. Run `pergamon export obsidian` first."
          );
        }
      },
    });

    // Command: Show stats
    this.addCommand({
      id: "show-stats",
      name: "Show pergamon stats",
      callback: async () => {
        await this.ensureManifest();
        const manifest = this.dataBridge.getManifest();

        if (!manifest) {
          new Notice("No pergamon manifest found.");
          return;
        }

        const highlights = manifest.items.filter(
          (i) => i.item_type === "highlight-source"
        );
        const bookmarks = manifest.items.filter(
          (i) => i.item_type === "bookmark"
        );
        const totalHighlights = highlights.reduce(
          (sum, i) => sum + i.highlight_count,
          0
        );

        new Notice(
          `Pergamon: ${manifest.item_count} items\n` +
            `  ${highlights.length} sources (${totalHighlights} highlights)\n` +
            `  ${bookmarks.length} bookmarks\n` +
            `  Last export: ${manifest.exported_at}`
        );
      },
    });

    // Auto-load manifest on startup (after vault is ready)
    this.app.workspace.onLayoutReady(async () => {
      await this.dataBridge.loadManifest();
    });
  }

  onunload(): void {
    // No cleanup needed — Obsidian handles command/ribbon removal.
  }

  async loadSettings(): Promise<void> {
    this.settings = Object.assign({}, DEFAULT_SETTINGS, await this.loadData());
  }

  async saveSettings(): Promise<void> {
    await this.saveData(this.settings);
  }

  /** Add or remove the ribbon icon based on settings. */
  updateRibbonIcon(): void {
    if (this.ribbonIconEl) {
      this.ribbonIconEl.remove();
      this.ribbonIconEl = null;
    }

    if (this.settings.showRibbonIcon) {
      this.ribbonIconEl = this.addRibbonIcon(
        "book-open",
        "Browse pergamon items",
        async () => {
          // Same as the browse command
          await this.ensureManifest();
          const items = this.dataBridge.getItems();

          if (items.length === 0) {
            new Notice("No pergamon items found. Run `pergamon export obsidian` first.");
            return;
          }

          new BrowseModal(
            this.app,
            items,
            this.settings.insertFormat,
            (item) => {
              this.app.workspace.openLinkText(item.file_path, "", false);
            }
          ).open();
        }
      );
    }
  }

  /** Load the manifest if it hasn't been loaded yet. */
  private async ensureManifest(): Promise<void> {
    if (!this.dataBridge.getManifest()) {
      await this.dataBridge.loadManifest();
    }
  }
}
