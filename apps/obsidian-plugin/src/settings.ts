/**
 * Plugin settings and settings tab.
 */

import { App, PluginSettingTab, Setting } from "obsidian";
import type PergamonPlugin from "./main";

/** Insert format options. */
export type InsertFormat = "wikilink" | "markdown" | "embed";

/** Persisted settings for the Pergamon plugin. */
export interface PergamonSettings {
  /** Folder within the vault where pergamon exports notes. */
  folderName: string;
  /** How to format inserted references. */
  insertFormat: InsertFormat;
  /** Whether to show the ribbon icon. */
  showRibbonIcon: boolean;
}

export const DEFAULT_SETTINGS: PergamonSettings = {
  folderName: "Pergamon",
  insertFormat: "wikilink",
  showRibbonIcon: true,
};

export class PergamonSettingTab extends PluginSettingTab {
  plugin: PergamonPlugin;

  constructor(app: App, plugin: PergamonPlugin) {
    super(app, plugin);
    this.plugin = plugin;
  }

  display(): void {
    const { containerEl } = this;
    containerEl.empty();

    new Setting(containerEl)
      .setName("Pergamon folder")
      .setDesc(
        "Folder within the vault where pergamon exports notes. " +
          "Must match the --folder flag used with `pergamon export obsidian`."
      )
      .addText((text) =>
        text
          .setPlaceholder("Pergamon")
          .setValue(this.plugin.settings.folderName)
          .onChange(async (value) => {
            this.plugin.settings.folderName = value || "Pergamon";
            this.plugin.dataBridge.setFolderName(
              this.plugin.settings.folderName
            );
            await this.plugin.saveSettings();
          })
      );

    new Setting(containerEl)
      .setName("Insert format")
      .setDesc("How references are inserted into notes.")
      .addDropdown((dropdown) =>
        dropdown
          .addOption("wikilink", "Wikilink — [[note|title]]")
          .addOption("markdown", "Markdown — [title](path)")
          .addOption("embed", "Embed — ![[note]]")
          .setValue(this.plugin.settings.insertFormat)
          .onChange(async (value) => {
            this.plugin.settings.insertFormat = value as InsertFormat;
            await this.plugin.saveSettings();
          })
      );

    new Setting(containerEl)
      .setName("Show ribbon icon")
      .setDesc("Show the Pergamon icon in the left ribbon.")
      .addToggle((toggle) =>
        toggle
          .setValue(this.plugin.settings.showRibbonIcon)
          .onChange(async (value) => {
            this.plugin.settings.showRibbonIcon = value;
            await this.plugin.saveSettings();
            this.plugin.updateRibbonIcon();
          })
      );
  }
}
