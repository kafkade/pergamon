//! Terminal event handling for the TUI.

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use pergamon_core::status::DocumentStatus;
use pergamon_storage::Database;

use super::app::{App, View};

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

    match app.view {
        View::ItemList => handle_list_keys(app, db, key.code),
        View::Reader => Ok(handle_reader_keys(app, key.code)),
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
        KeyCode::Enter => {
            app.open_reader();
            Ok(Action::None)
        }
        KeyCode::Char('r') => change_status(app, db, DocumentStatus::Reading),
        KeyCode::Char('s') => change_status(app, db, DocumentStatus::Reference),
        KeyCode::Char('a') => change_status(app, db, DocumentStatus::Archived),
        KeyCode::Char('d') => change_status(app, db, DocumentStatus::Discarded),
        KeyCode::Char('l') => change_status(app, db, DocumentStatus::Later),
        _ => Ok(Action::None),
    }
}

/// Handle key events in the article reader view.
const fn handle_reader_keys(app: &mut App, code: KeyCode) -> Action {
    match code {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.close_reader();
            Action::None
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.scroll_down(1);
            Action::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.scroll_up(1);
            Action::None
        }
        KeyCode::Char(' ') | KeyCode::PageDown => {
            app.scroll_down(20);
            Action::None
        }
        KeyCode::PageUp => {
            app.scroll_up(20);
            Action::None
        }
        KeyCode::Char('g') | KeyCode::Home => {
            app.scroll = 0;
            Action::None
        }
        KeyCode::Char('G') | KeyCode::End => {
            app.scroll = u16::MAX;
            Action::None
        }
        _ => Action::None,
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
