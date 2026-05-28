//! Application state for the TUI.

use pergamon_core::model::ContentItem;

/// Which view the TUI is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Inbox list of content items.
    ItemList,
    /// Article reader (full-screen scrollable text).
    Reader,
}

/// Top-level application state.
pub struct App {
    /// Current view mode.
    pub view: View,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Content items loaded for the list view.
    pub items: Vec<ContentItem>,
    /// Index of the selected item in the list.
    pub selected: usize,
    /// Vertical scroll position in the reader.
    pub scroll: u16,
    /// Whether to show the help overlay.
    pub show_help: bool,
    /// Status message (shown in the bottom bar).
    pub status_message: Option<String>,
}

impl App {
    /// Create a new `App` with the given content items.
    pub const fn new(items: Vec<ContentItem>) -> Self {
        Self {
            view: View::ItemList,
            should_quit: false,
            items,
            selected: 0,
            scroll: 0,
            show_help: false,
            status_message: None,
        }
    }

    /// Move selection up in the list.
    pub const fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move selection down in the list.
    pub fn move_down(&mut self) {
        if !self.items.is_empty() && self.selected < self.items.len() - 1 {
            self.selected += 1;
        }
    }

    /// Open the selected item in the reader.
    pub fn open_reader(&mut self) {
        if !self.items.is_empty() {
            self.view = View::Reader;
            self.scroll = 0;
        }
    }

    /// Go back to the list view.
    pub const fn close_reader(&mut self) {
        self.view = View::ItemList;
        self.scroll = 0;
    }

    /// Scroll the reader view up.
    pub const fn scroll_up(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_sub(amount);
    }

    /// Scroll the reader view down.
    pub const fn scroll_down(&mut self, amount: u16) {
        self.scroll = self.scroll.saturating_add(amount);
    }

    /// Get the currently selected item, if any.
    pub fn selected_item(&self) -> Option<&ContentItem> {
        self.items.get(self.selected)
    }

    /// Set a status message that will be shown in the status bar.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some(msg.into());
    }

    /// Clear the status message.
    pub fn clear_status(&mut self) {
        self.status_message = None;
    }
}
