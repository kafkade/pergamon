//! TUI rendering using ratatui.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};

use super::app::{App, View};

/// Render the current application state to a terminal frame.
pub fn render(app: &App, frame: &mut Frame<'_>) {
    match app.view {
        View::ItemList => render_list(app, frame),
        View::Reader => render_reader(app, frame),
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

            Row::new(vec![
                Cell::from(format!("{:>3}", i + 1)),
                Cell::from(item.title.clone()),
                Cell::from(item.content_type.as_str().to_owned()),
                Cell::from(item.status.as_str().to_owned()),
            ])
            .style(style)
        })
        .collect();

    let item_count = app.items.len();
    let title = format!(" Inbox ({item_count} items) ");

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
            .title_bottom(" ? help  q quit "),
    );

    frame.render_widget(table, layout[0]);

    // Status bar
    let status_text = app
        .status_message
        .as_deref()
        .unwrap_or("j/k navigate  Enter open  r read  s star  a archive  d discard");
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
        Constraint::Length(3), // title bar
        Constraint::Min(5),    // content
        Constraint::Length(1), // status bar
    ])
    .split(area);

    // Title block
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

    // Status bar
    let status = Paragraph::new(Span::styled(
        " Esc back  j/k scroll  Space page down  g/G top/bottom ",
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(status, layout[2]);
}

/// Render a centered help overlay.
fn render_help_overlay(frame: &mut Frame<'_>) {
    let area = frame.area();

    // Center a box in the middle of the screen.
    let width = 50.min(area.width.saturating_sub(4));
    let height = 16.min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    // Clear the area behind the popup.
    frame.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::styled("Keybindings", Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::from("  j / ↓        Move down"),
        Line::from("  k / ↑        Move up"),
        Line::from("  Enter        Open article"),
        Line::from("  Esc / q      Back / quit"),
        Line::from(""),
        Line::from("  r            Mark as Reading"),
        Line::from("  l            Mark as Later"),
        Line::from("  s            Star (Reference)"),
        Line::from("  a            Archive"),
        Line::from("  d            Discard"),
        Line::from(""),
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
