//! TUI rendering using ratatui.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use super::app::{App, PickerMode, View};

/// Render the current application state to a terminal frame.
pub fn render(app: &App, frame: &mut Frame<'_>) {
    match app.view {
        View::ItemList => render_list(app, frame),
        View::Reader => render_reader(app, frame),
    }

    if app.show_picker {
        render_picker(app, frame);
    }

    if let Some(ref dialog) = app.confirm {
        render_confirm(dialog, frame);
    }

    if app.show_help {
        render_help_overlay(frame);
    }
}

/// Render the inbox item list.
fn render_list(app: &App, frame: &mut Frame<'_>) {
    let area = frame.area();

    let layout = Layout::vertical([
        Constraint::Min(3),    // main table
        Constraint::Length(1), // status bar
    ])
    .split(area);

    // Header row
    let header = Row::new(vec![
        Cell::from(" # "),
        Cell::from("Title"),
        Cell::from("Type"),
        Cell::from("Status"),
    ])
    .style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    // Data rows
    let rows: Vec<Row<'_>> = app
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let style = if i == app.selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            let status_style = match item.status {
                pergamon_core::status::DocumentStatus::Inbox => Style::default().fg(Color::White),
                pergamon_core::status::DocumentStatus::Later => Style::default().fg(Color::Blue),
                pergamon_core::status::DocumentStatus::Reading => Style::default().fg(Color::Cyan),
                pergamon_core::status::DocumentStatus::Reference => {
                    Style::default().fg(Color::Yellow)
                }
                pergamon_core::status::DocumentStatus::Archived => {
                    Style::default().fg(Color::DarkGray)
                }
                pergamon_core::status::DocumentStatus::Discarded => Style::default().fg(Color::Red),
            };

            Row::new(vec![
                Cell::from(format!("{:>3}", i + 1)),
                Cell::from(item.title.clone()),
                Cell::from(item.content_type.as_str().to_owned()),
                Cell::from(Span::styled(item.status.as_str().to_owned(), status_style)),
            ])
            .style(style)
        })
        .collect();

    let item_count = app.items.len();
    let filter_label = app.filter_label();
    let title = format!(" {filter_label} ({item_count} items) ");

    // Right-side title with unread count.
    let unread_label = format!(" 📬 {} unread ", app.unread_count);

    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Min(20),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_bottom(unread_label),
    );

    frame.render_widget(table, layout[0]);

    // Status bar
    let status_text = app.status_message.as_deref().unwrap_or(
        "j/k nav  Enter open  r read  s star  a archive  d discard  f feed  Tab filter  ? help",
    );
    let status = Paragraph::new(Span::styled(
        status_text,
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(status, layout[1]);
}

/// Render the article reader view.
fn render_reader(app: &App, frame: &mut Frame<'_>) {
    let area = frame.area();

    let Some(item) = app.selected_item() else {
        let msg = Paragraph::new("No item selected.")
            .block(Block::default().borders(Borders::ALL).title(" Reader "));
        frame.render_widget(msg, area);
        return;
    };

    let layout = Layout::vertical([
        Constraint::Length(4), // title bar (with URL)
        Constraint::Min(5),    // content
        Constraint::Length(1), // status bar
    ])
    .split(area);

    // Title block with URL.
    let mut title_lines = vec![Line::styled(
        item.title.clone(),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(ref author) = item.author {
        title_lines.push(Line::styled(
            format!("by {author}"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(ref url) = item.url {
        title_lines.push(Line::styled(
            url.clone(),
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::UNDERLINED),
        ));
    }
    let title_block =
        Paragraph::new(Text::from(title_lines)).block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title_block, layout[0]);

    // Article content
    let content = item
        .content_text
        .as_deref()
        .unwrap_or("[No content available]");
    let paragraph = Paragraph::new(content)
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0))
        .block(Block::default());
    frame.render_widget(paragraph, layout[1]);

    // Status bar with item status.
    let status_hint = format!(
        " [{}]  Esc back  j/k scroll  Space pgdn  o open  r/s/a/d/l triage ",
        item.status.as_str()
    );
    let status = Paragraph::new(Span::styled(
        status_hint,
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(status, layout[2]);
}

/// Render the feed/folder picker overlay.
fn render_picker(app: &App, frame: &mut Frame<'_>) {
    let area = frame.area();

    let (title, items): (&str, Vec<String>) = match app.picker_mode {
        PickerMode::Feed => (
            " Select Feed (Tab → folders) ",
            app.feeds.iter().map(|f| f.title.clone()).collect(),
        ),
        PickerMode::Folder => (
            " Select Folder (Tab → feeds) ",
            app.folders.iter().map(|f| f.name.clone()).collect(),
        ),
    };

    let width = 50.min(area.width.saturating_sub(4));
    let max_visible = 20_u16;
    let item_len = items.len().min(usize::from(max_visible));
    #[allow(clippy::cast_possible_truncation)]
    let height = (item_len as u16 + 3)
        .min(max_visible)
        .min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup_area);

    if items.is_empty() {
        let empty = Paragraph::new("  (none)").block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().bg(Color::Black)),
        );
        frame.render_widget(empty, popup_area);
        return;
    }

    let lines: Vec<Line<'_>> = items
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let prefix = if i == app.picker_selected {
                "▸ "
            } else {
                "  "
            };
            let style = if i == app.picker_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::styled(format!("{prefix}{name}"), style)
        })
        .collect();

    let picker = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title)
            .style(Style::default().bg(Color::Black)),
    );

    frame.render_widget(picker, popup_area);
}

/// Render a confirmation dialog.
fn render_confirm(dialog: &super::app::ConfirmDialog, frame: &mut Frame<'_>) {
    let area = frame.area();

    let width = 50.min(area.width.saturating_sub(4));
    let height = 5.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup_area);

    let lines = vec![
        Line::from(""),
        Line::styled(
            dialog.message.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from(""),
        Line::styled(
            "  y/Enter confirm    n/Esc cancel",
            Style::default().fg(Color::DarkGray),
        ),
    ];

    let confirm = Paragraph::new(Text::from(lines)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Confirm ")
            .style(Style::default().bg(Color::Black).fg(Color::Yellow)),
    );

    frame.render_widget(confirm, popup_area);
}

/// Render a centered help overlay.
fn render_help_overlay(frame: &mut Frame<'_>) {
    let area = frame.area();

    // Center a box in the middle of the screen.
    let width = 56.min(area.width.saturating_sub(4));
    let height = 26.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    // Clear the area behind the popup.
    frame.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::styled("Keybindings", Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::styled(
            "  Navigation",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::from("  j / ↓        Move down"),
        Line::from("  k / ↑        Move up"),
        Line::from("  g / Home     Jump to top"),
        Line::from("  G / End      Jump to bottom"),
        Line::from("  Enter        Open article"),
        Line::from("  Esc / q      Back / quit"),
        Line::from(""),
        Line::styled("  Triage", Style::default().add_modifier(Modifier::BOLD)),
        Line::from("  r            Mark as Reading"),
        Line::from("  l            Mark as Later"),
        Line::from("  s            Star (Reference)"),
        Line::from("  a            Archive"),
        Line::from("  d            Discard"),
        Line::from("  R            Bulk mark as read"),
        Line::from("  o            Open in browser"),
        Line::from(""),
        Line::styled("  Filters", Style::default().add_modifier(Modifier::BOLD)),
        Line::from("  f            Filter by feed"),
        Line::from("  F            Filter by folder"),
        Line::from("  Tab          Cycle status filter"),
        Line::from("  1-5          Quick filter (inbox/later/…)"),
        Line::from("  0            Show all"),
        Line::from("  ?            Toggle this help"),
    ];

    let help = Paragraph::new(Text::from(help_text)).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Help ")
            .style(Style::default().bg(Color::Black)),
    );

    frame.render_widget(help, popup_area);
}
