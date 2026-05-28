//! Application state for the TUI.

use pergamon_core::model::{ContentItem, Feed, FeedFolder};
use pergamon_core::status::DocumentStatus;
use uuid::Uuid;

/// Which view the TUI is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    /// Inbox list of content items.
    ItemList,
    /// Article reader (full-screen scrollable text).
    Reader,
}

/// Active filter controlling which items are shown.
#[derive(Debug, Clone)]
pub enum FilterMode {
    /// All items regardless of status.
    All,
    /// Items with a specific status.
    Status(DocumentStatus),
    /// Items from a specific feed.
    Feed(Uuid, String),
    /// Items from feeds in a specific folder.
    Folder(Uuid, String),
    /// Full-text search results.
    Search(String),
}

impl std::fmt::Display for FilterMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => write!(f, "All"),
            Self::Status(s) => write!(f, "{s}"),
            Self::Feed(_, name) => write!(f, "Feed: {name}"),
            Self::Folder(_, name) => write!(f, "Folder: {name}"),
            Self::Search(query) => write!(f, "Search: {query}"),
        }
    }
}

/// What the feed/folder picker is currently displaying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    /// Showing feeds.
    Feed,
    /// Showing folders.
    Folder,
}

/// A pending confirmation dialog.
#[derive(Debug, Clone)]
pub struct ConfirmDialog {
    /// Message to display.
    pub message: String,
    /// Number of items that would be affected.
    #[allow(dead_code)]
    pub count: u64,
    /// Action to take on confirmation.
    pub action: ConfirmAction,
}

/// What to do when a confirmation is accepted.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    /// Bulk mark items as read (archived).
    BulkMarkRead,
}

/// Top-level application state.
#[allow(clippy::struct_excessive_bools)]
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
    /// Current filter mode.
    pub filter: FilterMode,
    /// All subscribed feeds (for the feed picker).
    pub feeds: Vec<Feed>,
    /// All feed folders (for the folder picker).
    pub folders: Vec<FeedFolder>,
    /// Whether the feed/folder picker overlay is visible.
    pub show_picker: bool,
    /// What the picker is showing (feeds or folders).
    pub picker_mode: PickerMode,
    /// Currently selected index in the picker.
    pub picker_selected: usize,
    /// Unread (inbox) item count for the status bar.
    pub unread_count: u64,
    /// Total item count matching the current filter.
    pub total_count: u64,
    /// Pending confirmation dialog.
    pub confirm: Option<ConfirmDialog>,
    /// Whether the search input bar is visible.
    pub show_search_input: bool,
    /// Text being typed in the search input bar.
    pub search_input: String,
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
            filter: FilterMode::Status(DocumentStatus::Inbox),
            feeds: Vec::new(),
            folders: Vec::new(),
            show_picker: false,
            picker_mode: PickerMode::Feed,
            picker_selected: 0,
            unread_count: 0,
            total_count: 0,
            confirm: None,
            show_search_input: false,
            search_input: String::new(),
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

    /// Move picker selection up.
    pub const fn picker_up(&mut self) {
        if self.picker_selected > 0 {
            self.picker_selected -= 1;
        }
    }

    /// Move picker selection down.
    pub const fn picker_down(&mut self, max: usize) {
        if self.picker_selected < max.saturating_sub(1) {
            self.picker_selected += 1;
        }
    }

    /// Clamp the selected index to the current items length.
    pub fn clamp_selection(&mut self) {
        if self.items.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.items.len() {
            self.selected = self.items.len() - 1;
        }
    }

    /// Filter label for the status bar.
    #[must_use]
    pub fn filter_label(&self) -> String {
        self.filter.to_string()
    }
}
