//! TUI module for pergamon.
//!
//! Implements a ratatui-based terminal interface with vim-style
//! keybindings for browsing the inbox and reading articles.

pub mod app;
pub mod event;
pub mod ui;

use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{Event, KeyCode, KeyModifiers};
use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
use pergamon_core::fsrs::{MemoryState, Parameters, Rating, Scheduler};
use pergamon_core::model::{ReviewCard, ReviewLog, ReviewStatsReport};
use pergamon_storage::Database;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Wrap};
use time::OffsetDateTime;
use uuid::Uuid;

/// Run a standalone TUI review session.
#[allow(clippy::too_many_lines)]
pub fn run_review(db: &Database, cards: Vec<ReviewCard>) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)
        .context("failed to enter alternate screen")?;

    let _guard = ReviewTermGuard;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).context("failed to initialize terminal")?;

    let scheduler = Scheduler::new(&Parameters::default());
    let total = cards.len();
    let mut reviewed = 0_usize;
    let mut again_count = 0_usize;

    let mut queue: Vec<ReviewCard> = cards;
    let mut current_idx = 0_usize;
    let mut show_answer = false;

    while current_idx < queue.len() {
        let card = &queue[current_idx];

        // Fetch the highlight text for display.
        let (quote_text, source_title) = {
            let highlight = db.get_highlight_meta(card.content_item_id).ok();
            let quote = highlight
                .as_ref()
                .map_or_else(|| "(no text)".to_owned(), |h| h.quote_text.clone());
            let note = highlight.as_ref().and_then(|h| h.note.clone());
            let source = highlight
                .as_ref()
                .and_then(|h| h.source_item_id)
                .and_then(|sid| db.get_content_item(sid).ok())
                .map(|item| item.title);
            let display = if let Some(n) = note {
                format!("{quote}\n\n  Note: {n}")
            } else {
                quote
            };
            (display, source.unwrap_or_default())
        };

        // Draw
        terminal
            .draw(|frame| {
                render_review(
                    frame,
                    &ReviewRender {
                        quote_text: &quote_text,
                        source_title: &source_title,
                        card,
                        show_answer,
                        reviewed,
                        total,
                        again_count,
                    },
                );
            })
            .context("failed to draw review UI")?;

        // Handle input
        if !crossterm::event::poll(Duration::from_millis(100))? {
            continue;
        }

        let Event::Key(key) = crossterm::event::read()? else {
            continue;
        };

        // Ctrl+C or 'q' to quit
        if (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
            || key.code == KeyCode::Char('q')
        {
            break;
        }

        if !show_answer {
            // Space or Enter to reveal answer
            if matches!(key.code, KeyCode::Char(' ') | KeyCode::Enter) {
                show_answer = true;
            }
            continue;
        }

        // Rating keys: 1=Again, 2=Hard, 3=Good, 4=Easy
        let rating = match key.code {
            KeyCode::Char('1') => Some(Rating::Again),
            KeyCode::Char('2') => Some(Rating::Hard),
            KeyCode::Char('3') => Some(Rating::Good),
            KeyCode::Char('4') => Some(Rating::Easy),
            _ => None,
        };

        if let Some(rating) = rating {
            let now = OffsetDateTime::now_utc();
            let elapsed_days = card.last_reviewed_at.map_or(0.0, |last| {
                let dur = now - last;
                dur.as_seconds_f64() / 86400.0
            });

            let memory = match (card.stability, card.difficulty) {
                (Some(s), Some(d)) => Some(MemoryState {
                    stability: s,
                    difficulty: d,
                }),
                _ => None,
            };

            let output = scheduler.schedule(card.state, memory, elapsed_days, rating);

            let due_at = now + time::Duration::seconds_f64(output.scheduled_days * 86400.0);
            let new_review_count = card.review_count + 1;
            let new_lapse_count = if rating == Rating::Again {
                card.lapse_count + 1
            } else {
                card.lapse_count
            };

            // Update the card in the database
            db.update_review_card(
                card.id,
                output.next_state.as_str(),
                output.memory.stability,
                output.memory.difficulty,
                due_at,
                now,
                new_review_count,
                new_lapse_count,
                output.scheduled_days,
            )
            .context("failed to update review card")?;

            // Insert review log
            let log = ReviewLog {
                id: Uuid::new_v4(),
                card_id: card.id,
                rating,
                state_before: card.state,
                stability_before: card.stability,
                difficulty_before: card.difficulty,
                state_after: output.next_state,
                stability_after: output.memory.stability,
                difficulty_after: output.memory.difficulty,
                elapsed_days,
                scheduled_days: output.scheduled_days,
                reviewed_at: now,
            };
            db.insert_review_log(&log)
                .context("failed to insert review log")?;

            reviewed += 1;
            if rating == Rating::Again {
                again_count += 1;
                // Re-enqueue lapsed card at the end
                let mut updated_card = queue[current_idx].clone();
                updated_card.state = output.next_state;
                updated_card.stability = Some(output.memory.stability);
                updated_card.difficulty = Some(output.memory.difficulty);
                updated_card.due_at = due_at;
                updated_card.last_reviewed_at = Some(now);
                updated_card.review_count = new_review_count;
                updated_card.lapse_count = new_lapse_count;
                updated_card.scheduled_days = Some(output.scheduled_days);
                queue.push(updated_card);
            }

            current_idx += 1;
            show_answer = false;
        }
    }

    // Show summary, allow 's' for stats or any other key to exit
    let mut show_stats = false;
    loop {
        terminal
            .draw(|frame| {
                if show_stats {
                    if let Ok(report) = db.review_stats_report(OffsetDateTime::now_utc()) {
                        render_stats_dashboard(frame, &report);
                    } else {
                        render_review_summary(frame, reviewed, again_count);
                    }
                } else {
                    render_review_summary(frame, reviewed, again_count);
                }
            })
            .context("failed to draw summary")?;

        if crossterm::event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = crossterm::event::read()?
        {
            match key.code {
                KeyCode::Char('s') if !show_stats => {
                    show_stats = true;
                }
                _ => break,
            }
        }
    }

    Ok(())
}

/// State tracked during a review session for rendering.
struct ReviewRender<'a> {
    quote_text: &'a str,
    source_title: &'a str,
    card: &'a ReviewCard,
    show_answer: bool,
    reviewed: usize,
    total: usize,
    again_count: usize,
}

fn render_review(frame: &mut ratatui::Frame, state: &ReviewRender<'_>) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(5),    // card content
            Constraint::Length(3), // controls
            Constraint::Length(1), // progress bar
        ])
        .split(area);

    // Header
    let header_text = format!(
        "Review Session — {}/{} reviewed | {} lapsed",
        state.reviewed, state.total, state.again_count
    );
    let header = Paragraph::new(header_text)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(header, chunks[0]);

    // Card content
    let card_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " {} | state: {} ",
            short_id(state.card.id),
            state.card.state
        ))
        .title_alignment(Alignment::Left);

    let inner = card_block.inner(chunks[1]);
    frame.render_widget(card_block, chunks[1]);

    let content_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // source
            Constraint::Length(1), // spacer
            Constraint::Min(3),    // quote
        ])
        .split(inner);

    // Source title
    if !state.source_title.is_empty() {
        let src = Paragraph::new(Line::from(vec![
            Span::styled("Source: ", Style::default().fg(Color::DarkGray)),
            Span::styled(state.source_title, Style::default().fg(Color::Cyan)),
        ]));
        frame.render_widget(src, content_chunks[0]);
    }

    // Quote text (always visible)
    let quote = Paragraph::new(Text::from(state.quote_text))
        .wrap(Wrap { trim: false })
        .style(Style::default().add_modifier(Modifier::ITALIC));
    frame.render_widget(quote, content_chunks[2]);

    // Controls
    let controls = if state.show_answer {
        Paragraph::new(Line::from(vec![
            Span::styled("[1] Again  ", Style::default().fg(Color::Red)),
            Span::styled("[2] Hard  ", Style::default().fg(Color::Yellow)),
            Span::styled("[3] Good  ", Style::default().fg(Color::Green)),
            Span::styled("[4] Easy  ", Style::default().fg(Color::Cyan)),
            Span::raw("  [q] Quit"),
        ]))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::TOP))
    } else {
        Paragraph::new("[Space/Enter] Show answer  [q] Quit")
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::TOP))
    };
    frame.render_widget(controls, chunks[2]);

    // Progress bar
    #[allow(clippy::cast_precision_loss)]
    let progress = if state.total > 0 {
        state.reviewed as f64 / state.total as f64
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .ratio(progress.min(1.0))
        .gauge_style(Style::default().fg(Color::Green));
    frame.render_widget(gauge, chunks[3]);
}

fn render_review_summary(frame: &mut ratatui::Frame, reviewed: usize, again_count: usize) {
    let area = frame.area();
    let center = centered_rect(40, 10, area);

    let success = reviewed.saturating_sub(again_count);
    #[allow(clippy::cast_precision_loss)]
    let retention = if reviewed > 0 {
        success as f64 / reviewed as f64 * 100.0
    } else {
        0.0
    };

    let text = vec![
        Line::raw(""),
        Line::styled(
            "Session Complete!",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(format!("  Reviewed:   {reviewed}")),
        Line::raw(format!("  Correct:    {success}")),
        Line::raw(format!("  Lapsed:     {again_count}")),
        Line::raw(format!("  Retention:  {retention:.0}%")),
        Line::raw(""),
        Line::styled(
            "[s] Stats dashboard  [any key] Exit",
            Style::default().fg(Color::DarkGray),
        ),
    ];

    let summary = Paragraph::new(text).alignment(Alignment::Center).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Review Summary "),
    );
    frame.render_widget(summary, center);
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width.min(area.width), height.min(area.height))
}

fn short_id(id: Uuid) -> String {
    id.to_string()[..8].to_owned()
}

/// Run a standalone TUI stats dashboard (no review session needed).
pub fn run_stats_tui(db: &Database) -> Result<()> {
    let report = db
        .review_stats_report(OffsetDateTime::now_utc())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    enable_raw_mode().context("failed to enable raw mode")?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)
        .context("failed to enter alternate screen")?;

    let _guard = ReviewTermGuard;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).context("failed to initialize terminal")?;

    loop {
        terminal
            .draw(|frame| {
                render_stats_dashboard(frame, &report);
            })
            .context("failed to draw stats")?;

        if crossterm::event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = crossterm::event::read()?
            && (key.code == KeyCode::Char('q')
                || key.code == KeyCode::Esc
                || (key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')))
        {
            break;
        }
    }

    Ok(())
}

/// Render the full stats dashboard as a TUI screen.
#[allow(clippy::too_many_lines)]
fn render_stats_dashboard(frame: &mut ratatui::Frame, report: &ReviewStatsReport) {
    let area = frame.area();

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // title
            Constraint::Length(12), // core stats + streaks
            Constraint::Min(6),     // charts
            Constraint::Length(1),  // footer
        ])
        .split(area);

    // Title
    let title = Paragraph::new(Line::styled(
        " Review Statistics Dashboard ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, main_chunks[0]);

    // Core stats + streaks (side by side)
    let stats = &report.stats;
    let top_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(35),
            Constraint::Percentage(30),
            Constraint::Percentage(35),
        ])
        .split(main_chunks[1]);

    // Cards overview
    let cards_text = vec![
        Line::raw(format!("Total cards:   {}", stats.total_cards)),
        Line::raw(format!("  New:         {}", stats.new_count)),
        Line::raw(format!("  Learning:    {}", stats.learning_count)),
        Line::raw(format!("  Review:      {}", stats.review_count)),
        Line::raw(format!("  Relearning:  {}", stats.relearning_count)),
        Line::raw(""),
        Line::raw(format!("Due now:       {}", stats.due_count)),
        Line::raw(format!("Today:         {}", stats.reviews_today)),
    ];
    let cards_panel = Paragraph::new(cards_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Cards ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(cards_panel, top_cols[0]);

    // Retention + streaks
    let retention_text = vec![
        Line::raw(format!(
            "Retention:  {:.1}%",
            stats.observed_retention * 100.0
        )),
        Line::raw(format!("Reviews:    {}", stats.total_reviews)),
        Line::raw(format!("Successes:  {}", stats.success_count)),
        Line::raw(""),
        Line::styled("Streaks", Style::default().add_modifier(Modifier::BOLD)),
        Line::raw(format!(
            "  Current:  {} day{}",
            stats.current_streak,
            if stats.current_streak == 1 { "" } else { "s" }
        )),
        Line::raw(format!(
            "  Longest:  {} day{}",
            stats.longest_streak,
            if stats.longest_streak == 1 { "" } else { "s" }
        )),
    ];
    let retention_panel = Paragraph::new(retention_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Retention ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(retention_panel, top_cols[1]);

    // Source breakdown
    let mut src_lines: Vec<Line<'_>> = Vec::new();
    if report.source_breakdown.is_empty() {
        src_lines.push(Line::styled(
            "(no review cards)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for src in &report.source_breakdown {
            src_lines.push(Line::raw(format!("  {:<12} {}", src.origin, src.count)));
        }
    }
    let src_panel = Paragraph::new(src_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Sources ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(src_panel, top_cols[2]);

    // Charts area: daily + weekly side by side
    let chart_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(main_chunks[2]);

    // Daily bar chart (last 7 days)
    let daily: Vec<_> = report.daily_history.iter().rev().take(7).collect();
    let max_daily = daily.iter().map(|d| d.reviews).max().unwrap_or(1).max(1);
    let mut daily_lines: Vec<Line<'_>> = Vec::new();
    for day in daily.into_iter().rev() {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let bar_len = ((day.reviews as f64 / max_daily as f64) * 15.0) as usize;
        let bar: String = "█".repeat(bar_len);
        let date_short = day.date.get(5..).unwrap_or(&day.date);
        daily_lines.push(Line::from(vec![
            Span::raw(format!("  {date_short} ")),
            Span::styled(
                format!("{:>3}", day.reviews),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" "),
            Span::styled(bar, Style::default().fg(Color::Green)),
        ]));
    }
    if daily_lines.is_empty() {
        daily_lines.push(Line::styled(
            "  (no reviews yet)",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let daily_panel = Paragraph::new(daily_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Last 7 Days ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(daily_panel, chart_cols[0]);

    // Weekly chart
    let max_weekly = report
        .weekly_history
        .iter()
        .map(|w| w.reviews)
        .max()
        .unwrap_or(1)
        .max(1);
    let mut weekly_lines: Vec<Line<'_>> = Vec::new();
    for week in &report.weekly_history {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let bar_len = ((week.reviews as f64 / max_weekly as f64) * 12.0) as usize;
        let bar: String = "█".repeat(bar_len);
        weekly_lines.push(Line::from(vec![
            Span::raw(format!("  {} ", week.week)),
            Span::styled(
                format!("{:>3}", week.reviews),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" "),
            Span::styled(bar, Style::default().fg(Color::Cyan)),
        ]));
    }
    if weekly_lines.is_empty() {
        weekly_lines.push(Line::styled(
            "  (no reviews yet)",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let weekly_panel = Paragraph::new(weekly_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Weekly Trend ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(weekly_panel, chart_cols[1]);

    // Footer
    let footer = Paragraph::new(Line::styled(
        "Press [q] or [Esc] to exit",
        Style::default().fg(Color::DarkGray),
    ))
    .alignment(Alignment::Center);
    frame.render_widget(footer, main_chunks[3]);
}

/// Guard that restores terminal state on drop for review sessions.
struct ReviewTermGuard;

impl Drop for ReviewTermGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
    }
}

// ======================================================================
// Usage statistics TUI
// ======================================================================

/// Run a standalone usage stats TUI dashboard.
pub fn run_usage_stats_tui(db: &Database) -> Result<()> {
    let report = db
        .usage_stats_report(OffsetDateTime::now_utc())
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    enable_raw_mode().context("failed to enable raw mode")?;
    crossterm::execute!(std::io::stdout(), EnterAlternateScreen)
        .context("failed to enter alternate screen")?;

    let _guard = ReviewTermGuard;

    let backend = CrosstermBackend::new(std::io::stdout());
    let mut terminal = Terminal::new(backend).context("failed to initialize terminal")?;

    loop {
        terminal
            .draw(|frame| {
                render_usage_dashboard(frame, &report);
            })
            .context("failed to draw usage stats")?;

        if crossterm::event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = crossterm::event::read()?
            && (key.code == KeyCode::Char('q')
                || key.code == KeyCode::Esc
                || (key.modifiers.contains(KeyModifiers::CONTROL)
                    && key.code == KeyCode::Char('c')))
        {
            break;
        }
    }

    Ok(())
}

/// Render the usage statistics dashboard.
#[allow(clippy::too_many_lines)]
fn render_usage_dashboard(
    frame: &mut ratatui::Frame,
    report: &pergamon_core::model::UsageStatsReport,
) {
    let area = frame.area();
    let o = &report.overview;

    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // title
            Constraint::Length(12), // overview panels
            Constraint::Min(8),     // charts
            Constraint::Length(1),  // footer
        ])
        .split(area);

    // Title
    let title = Paragraph::new(Line::styled(
        " Usage Statistics Dashboard ",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    ))
    .alignment(Alignment::Center)
    .block(Block::default().borders(Borders::BOTTOM));
    frame.render_widget(title, main_chunks[0]);

    // Overview panels (3 columns)
    let top_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(main_chunks[1]);

    // Library panel
    let lib_text = vec![
        Line::raw(format!("Total items:  {}", o.total_items)),
        Line::raw(format!("  Inbox:      {}", o.inbox_count)),
        Line::raw(format!("  Archived:   {}", o.archived_count)),
        Line::raw(format!("  Highlights: {}", o.total_highlights)),
        Line::raw(format!("  Feeds:      {}", o.total_feeds)),
    ];
    let lib_panel = Paragraph::new(lib_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Library ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(lib_panel, top_cols[0]);

    // Save rate panel
    let save_text = vec![
        Line::raw(format!("Today:       {}", o.items_saved_today)),
        Line::raw(format!("This week:   {}", o.items_saved_this_week)),
        Line::raw(format!("This month:  {}", o.items_saved_this_month)),
        Line::raw(format!("30-day avg:  {:.1}/day", o.saves_per_day_30d)),
        Line::raw(format!("HL rate:     {:.1}%", o.highlight_rate * 100.0)),
    ];
    let save_panel = Paragraph::new(save_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Save Rate ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(save_panel, top_cols[1]);

    // Reading panel
    let read_text = vec![
        Line::raw(format!(
            "Read time:   {} hr {} min",
            o.total_reading_minutes / 60,
            o.total_reading_minutes % 60
        )),
        Line::raw(format!(
            "Streak:      {} day{}",
            o.reading_streak_days,
            if o.reading_streak_days == 1 { "" } else { "s" }
        )),
        Line::raw(format!(
            "Longest:     {} day{}",
            o.longest_reading_streak,
            if o.longest_reading_streak == 1 {
                ""
            } else {
                "s"
            }
        )),
    ];
    let read_panel = Paragraph::new(read_text).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Reading ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(read_panel, top_cols[2]);

    // Charts area: daily activity + top sources
    let chart_cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(main_chunks[2]);

    // Daily bar chart (last 7 days)
    let daily: Vec<_> = report.reading_activity.daily.iter().rev().take(7).collect();
    let max_read = daily.iter().map(|d| d.items_read).max().unwrap_or(1).max(1);
    let mut daily_lines: Vec<Line<'_>> = Vec::new();
    for day in daily.into_iter().rev() {
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let bar_len = ((day.items_read as f64 / max_read as f64) * 15.0) as usize;
        let bar: String = "█".repeat(bar_len);
        let date_short = day.date.get(5..).unwrap_or(&day.date);
        daily_lines.push(Line::from(vec![
            Span::raw(format!("  {date_short} ")),
            Span::styled(
                format!("{:>3}", day.items_read),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(" "),
            Span::styled(bar, Style::default().fg(Color::Green)),
        ]));
    }
    if daily_lines.is_empty() {
        daily_lines.push(Line::styled(
            "  (no reading activity)",
            Style::default().fg(Color::DarkGray),
        ));
    }
    let daily_panel = Paragraph::new(daily_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Last 7 Days (read) ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(daily_panel, chart_cols[0]);

    // Top sources + tag distribution
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(chart_cols[1]);

    let mut src_lines: Vec<Line<'_>> = Vec::new();
    if report.top_sources.is_empty() {
        src_lines.push(Line::styled(
            "  (no sources yet)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for src in report.top_sources.iter().take(6) {
            let name: String = src.source_name.chars().take(20).collect();
            src_lines.push(Line::raw(format!(
                "  {:<20} {}/{}",
                name, src.items_read, src.total_items
            )));
        }
    }
    let src_panel = Paragraph::new(src_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Top Sources ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(src_panel, right_chunks[0]);

    let mut tag_lines: Vec<Line<'_>> = Vec::new();
    if report.tag_distribution.is_empty() {
        tag_lines.push(Line::styled(
            "  (no tags yet)",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        for tag in report.tag_distribution.iter().take(6) {
            tag_lines.push(Line::raw(format!("  {:<20} {}", tag.tag_name, tag.count)));
        }
    }
    let tag_panel = Paragraph::new(tag_lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Tags ")
            .title_alignment(Alignment::Left),
    );
    frame.render_widget(tag_panel, right_chunks[1]);

    // Footer
    let footer = Paragraph::new(Line::styled(
        "Press [q] or [Esc] to exit",
        Style::default().fg(Color::DarkGray),
    ))
    .alignment(Alignment::Center);
    frame.render_widget(footer, main_chunks[3]);
}
