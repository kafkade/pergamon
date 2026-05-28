//! Terminal event handling for the TUI.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::{ContentItemFilter, Database};

use super::app::{App, ConfirmAction, ConfirmDialog, FilterMode, PickerMode, View};

/// Action resulting from processing a key event.
pub enum Action {
    /// No state change needed.
    None,
    /// Status of an item was changed; reload items.
    Reload,
}

/// Poll for terminal events and handle them.
///
/// Returns `Ok(action)` indicating whether the caller needs to reload data.
pub fn handle_events(app: &mut App, db: &Database) -> Result<Action> {
    if !event::poll(Duration::from_millis(100))? {
        return Ok(Action::None);
    }

    let Event::Key(key) = event::read()? else {
        return Ok(Action::None);
    };

    // Ctrl+C always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.should_quit = true;
        return Ok(Action::None);
    }

    // Confirmation dialog takes priority.
    if app.confirm.is_some() {
        return handle_confirm_keys(app, db, key.code);
    }

    // Toggle help overlay.
    if key.code == KeyCode::Char('?') {
        app.show_help = !app.show_help;
        return Ok(Action::None);
    }

    // If help overlay is showing, any other key dismisses it.
    if app.show_help {
        app.show_help = false;
        return Ok(Action::None);
    }

    // Picker overlay.
    if app.show_picker {
        return Ok(handle_picker_keys(app, key.code));
    }

    match app.view {
        View::ItemList => handle_list_keys(app, db, key.code),
        View::Reader => handle_reader_keys(app, db, key.code),
    }
}

/// Handle key events in the item list view.
fn handle_list_keys(app: &mut App, db: &Database, code: KeyCode) -> Result<Action> {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
            Ok(Action::None)
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.move_down();
            app.clear_status();
            Ok(Action::None)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.move_up();
            app.clear_status();
            Ok(Action::None)
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.selected = 0;
            app.clear_status();
            Ok(Action::None)
        }
        KeyCode::Char('G') | KeyCode::End => {
            if !app.items.is_empty() {
                app.selected = app.items.len() - 1;
            }
            app.clear_status();
            Ok(Action::None)
        }
        KeyCode::Enter => {
            app.open_reader();
            Ok(Action::None)
        }
        // Triage actions.
        KeyCode::Char('r') => change_status(app, db, DocumentStatus::Reading),
        KeyCode::Char('s') => change_status(app, db, DocumentStatus::Reference),
        KeyCode::Char('a') => change_status(app, db, DocumentStatus::Archived),
        KeyCode::Char('d') => change_status(app, db, DocumentStatus::Discarded),
        KeyCode::Char('l') => change_status(app, db, DocumentStatus::Later),
        // Open in browser.
        KeyCode::Char('o') => {
            open_in_browser(app);
            Ok(Action::None)
        }
        // Feed/folder picker.
        KeyCode::Char('f') => {
            app.show_picker = true;
            app.picker_mode = PickerMode::Feed;
            app.picker_selected = 0;
            Ok(Action::None)
        }
        KeyCode::Char('F') => {
            app.show_picker = true;
            app.picker_mode = PickerMode::Folder;
            app.picker_selected = 0;
            Ok(Action::None)
        }
        // Status filter cycling with Tab.
        KeyCode::Tab => {
            cycle_filter(app);
            Ok(Action::Reload)
        }
        // Quick status filters.
        KeyCode::Char('1') => {
            app.filter = FilterMode::Status(DocumentStatus::Inbox);
            app.selected = 0;
            Ok(Action::Reload)
        }
        KeyCode::Char('2') => {
            app.filter = FilterMode::Status(DocumentStatus::Later);
            app.selected = 0;
            Ok(Action::Reload)
        }
        KeyCode::Char('3') => {
            app.filter = FilterMode::Status(DocumentStatus::Reading);
            app.selected = 0;
            Ok(Action::Reload)
        }
        KeyCode::Char('4') => {
            app.filter = FilterMode::Status(DocumentStatus::Reference);
            app.selected = 0;
            Ok(Action::Reload)
        }
        KeyCode::Char('5') => {
            app.filter = FilterMode::Status(DocumentStatus::Archived);
            app.selected = 0;
            Ok(Action::Reload)
        }
        // Clear filter.
        KeyCode::Char('0') => {
            app.filter = FilterMode::All;
            app.selected = 0;
            Ok(Action::Reload)
        }
        // Bulk mark as read.
        KeyCode::Char('R') => initiate_bulk_mark_read(app, db),
        _ => Ok(Action::None),
    }
}

/// Handle key events in the article reader view.
fn handle_reader_keys(app: &mut App, db: &Database, code: KeyCode) -> Result<Action> {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.close_reader();
            Ok(Action::None)
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.scroll_down(1);
            Ok(Action::None)
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll_up(1);
            Ok(Action::None)
        }
        KeyCode::Char(' ') | KeyCode::PageDown => {
            app.scroll_down(20);
            Ok(Action::None)
        }
        KeyCode::PageUp => {
            app.scroll_up(20);
            Ok(Action::None)
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.scroll = 0;
            Ok(Action::None)
        }
        KeyCode::Char('G') | KeyCode::End => {
            app.scroll = u16::MAX;
            Ok(Action::None)
        }
        // Triage actions work in reader too.
        KeyCode::Char('r') => change_status(app, db, DocumentStatus::Reading),
        KeyCode::Char('s') => change_status(app, db, DocumentStatus::Reference),
        KeyCode::Char('a') => change_status(app, db, DocumentStatus::Archived),
        KeyCode::Char('d') => change_status(app, db, DocumentStatus::Discarded),
        KeyCode::Char('l') => change_status(app, db, DocumentStatus::Later),
        // Open in browser.
        KeyCode::Char('o') => {
            open_in_browser(app);
            Ok(Action::None)
        }
        _ => Ok(Action::None),
    }
}

/// Handle key events in the feed/folder picker overlay.
fn handle_picker_keys(app: &mut App, code: KeyCode) -> Action {
    let item_count = match app.picker_mode {
        PickerMode::Feed => app.feeds.len(),
        PickerMode::Folder => app.folders.len(),
    };

    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.show_picker = false;
            Action::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.picker_down(item_count);
            Action::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.picker_up();
            Action::None
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.picker_selected = 0;
            Action::None
        }
        KeyCode::Char('G') | KeyCode::End => {
            if item_count > 0 {
                app.picker_selected = item_count - 1;
            }
            Action::None
        }
        KeyCode::Enter => {
            match app.picker_mode {
                PickerMode::Feed => {
                    if let Some(feed) = app.feeds.get(app.picker_selected) {
                        app.filter = FilterMode::Feed(feed.id, feed.title.clone());
                        app.selected = 0;
                        app.show_picker = false;
                        return Action::Reload;
                    }
                }
                PickerMode::Folder => {
                    if let Some(folder) = app.folders.get(app.picker_selected) {
                        app.filter = FilterMode::Folder(folder.id, folder.name.clone());
                        app.selected = 0;
                        app.show_picker = false;
                        return Action::Reload;
                    }
                }
            }
            Action::None
        }
        // Switch between feed and folder mode.
        KeyCode::Tab => {
            app.picker_mode = match app.picker_mode {
                PickerMode::Feed => PickerMode::Folder,
                PickerMode::Folder => PickerMode::Feed,
            };
            app.picker_selected = 0;
            Action::None
        }
        _ => Action::None,
    }
}

/// Handle key events in the confirmation dialog.
fn handle_confirm_keys(app: &mut App, db: &Database, code: KeyCode) -> Result<Action> {
    match code {
        KeyCode::Char('y') | KeyCode::Enter => {
            if let Some(dialog) = app.confirm.take() {
                match dialog.action {
                    ConfirmAction::BulkMarkRead => {
                        let filter = build_filter(&app.filter);
                        // Only mark inbox items.
                        let inbox_filter = ContentItemFilter {
                            status: Some(DocumentStatus::Inbox),
                            ..filter
                        };
                        let affected =
                            db.bulk_update_status(&inbox_filter, DocumentStatus::Archived)?;
                        app.set_status(format!("Marked {affected} item(s) as archived"));
                        return Ok(Action::Reload);
                    }
                }
            }
            Ok(Action::None)
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.confirm = None;
            app.set_status("Cancelled");
            Ok(Action::None)
        }
        _ => Ok(Action::None),
    }
}

/// Change the status of the selected item in the database.
fn change_status(app: &mut App, db: &Database, status: DocumentStatus) -> Result<Action> {
    let Some(item) = app.selected_item() else {
        return Ok(Action::None);
    };

    let id = item.id;
    let title = item.title.clone();
    db.update_content_item_status(id, status)?;
    app.set_status(format!("{title} → {status}"));
    Ok(Action::Reload)
}

/// Open the selected item's URL in the default browser.
fn open_in_browser(app: &mut App) {
    let Some(item) = app.selected_item() else {
        app.set_status("No item selected");
        return;
    };

    let Some(ref url) = item.url else {
        app.set_status("No URL for this item");
        return;
    };

    match open::that(url) {
        Ok(()) => app.set_status(format!("Opened: {url}")),
        Err(e) => app.set_status(format!("Failed to open browser: {e}")),
    }
}

/// Cycle through filter modes with Tab.
fn cycle_filter(app: &mut App) {
    app.filter = match &app.filter {
        FilterMode::All => FilterMode::Status(DocumentStatus::Inbox),
        FilterMode::Status(DocumentStatus::Inbox) => FilterMode::Status(DocumentStatus::Later),
        FilterMode::Status(DocumentStatus::Later) => FilterMode::Status(DocumentStatus::Reading),
        FilterMode::Status(DocumentStatus::Reading) => {
            FilterMode::Status(DocumentStatus::Reference)
        }
        FilterMode::Status(DocumentStatus::Reference) => {
            FilterMode::Status(DocumentStatus::Archived)
        }
        FilterMode::Status(DocumentStatus::Archived | DocumentStatus::Discarded)
        | FilterMode::Feed(..)
        | FilterMode::Folder(..) => FilterMode::All,
    };
    app.selected = 0;
}

/// Initiate bulk mark-as-read with confirmation.
fn initiate_bulk_mark_read(app: &mut App, db: &Database) -> Result<Action> {
    let filter = build_filter(&app.filter);
    let inbox_filter = ContentItemFilter {
        status: Some(DocumentStatus::Inbox),
        ..filter
    };
    let count = db.count_content_items_filtered(&inbox_filter)?;

    if count == 0 {
        app.set_status("No unread items to mark");
        return Ok(Action::None);
    }

    let scope = match &app.filter {
        FilterMode::All => "all feeds".to_owned(),
        FilterMode::Status(_) => "current view".to_owned(),
        FilterMode::Feed(_, name) => format!("feed \"{name}\""),
        FilterMode::Folder(_, name) => format!("folder \"{name}\""),
    };

    app.confirm = Some(ConfirmDialog {
        message: format!("Archive {count} unread item(s) in {scope}?"),
        count,
        action: ConfirmAction::BulkMarkRead,
    });

    Ok(Action::None)
}

/// Build a [`ContentItemFilter`] from the current [`FilterMode`].
pub fn build_filter(filter: &FilterMode) -> ContentItemFilter {
    match filter {
        FilterMode::All => ContentItemFilter::default(),
        FilterMode::Status(status) => ContentItemFilter {
            status: Some(*status),
            ..ContentItemFilter::default()
        },
        FilterMode::Feed(id, _) => ContentItemFilter {
            feed_id: Some(*id),
            ..ContentItemFilter::default()
        },
        FilterMode::Folder(id, _) => ContentItemFilter {
            folder_id: Some(*id),
            ..ContentItemFilter::default()
        },
    }
}
