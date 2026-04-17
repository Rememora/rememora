use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Frame, Terminal,
};
use rusqlite::Connection;
use std::io::stdout;

use rememora::hierarchy::{self, ScoredContext};
use rememora::models::context::{self, ContextRecord};
use rememora::models::project;
use rememora::search;

// ── Panel focus ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Panel {
    Projects,
    Memories,
    Detail,
}

impl Panel {
    fn next(self) -> Self {
        match self {
            Panel::Projects => Panel::Memories,
            Panel::Memories => Panel::Detail,
            Panel::Detail => Panel::Projects,
        }
    }

    fn prev(self) -> Self {
        match self {
            Panel::Projects => Panel::Detail,
            Panel::Memories => Panel::Projects,
            Panel::Detail => Panel::Memories,
        }
    }
}

// ── App state ────────────────────────────────────────────────────────────

struct App {
    focus: Panel,

    // Left panel: project list + category filter
    projects: Vec<String>,
    project_state: ListState,

    categories: Vec<String>,
    category_state: ListState,
    showing_categories: bool, // whether the left panel shows categories under projects

    // Right panel: memories
    memories: Vec<ScoredContext>,
    memory_state: ListState,

    // Bottom pane: detail
    detail_scroll: u16,

    // Search mode
    search_active: bool,
    search_query: String,
    search_results: Vec<search::SearchResult>,
    search_state: ListState,

    // Quit flag
    quit: bool,
}

impl App {
    fn new(conn: &Connection) -> Result<Self> {
        let mut projects = vec!["(all)".to_string()];
        let proj_records = project::list(conn)?;
        for p in &proj_records {
            projects.push(p.name.clone());
        }

        let categories = vec![
            "(all)".to_string(),
            "preference".to_string(),
            "entity".to_string(),
            "decision".to_string(),
            "event".to_string(),
            "case".to_string(),
            "pattern".to_string(),
        ];

        let mut project_state = ListState::default();
        project_state.select(Some(0));

        let mut category_state = ListState::default();
        category_state.select(Some(0));

        let memories = hierarchy::get_l0_map(conn, None)?;

        let mut memory_state = ListState::default();
        if !memories.is_empty() {
            memory_state.select(Some(0));
        }

        Ok(Self {
            focus: Panel::Projects,
            projects,
            project_state,
            categories,
            category_state,
            showing_categories: false,
            memories,
            memory_state,
            detail_scroll: 0,
            search_active: false,
            search_query: String::new(),
            search_results: Vec::new(),
            search_state: ListState::default(),
            quit: false,
        })
    }

    fn selected_project(&self) -> Option<&str> {
        let idx = self.project_state.selected().unwrap_or(0);
        if idx == 0 {
            None // "(all)"
        } else {
            self.projects.get(idx).map(|s| s.as_str())
        }
    }

    fn selected_category(&self) -> Option<&str> {
        if !self.showing_categories {
            return None;
        }
        let idx = self.category_state.selected().unwrap_or(0);
        if idx == 0 {
            None // "(all)"
        } else {
            self.categories.get(idx).map(|s| s.as_str())
        }
    }

    fn selected_memory(&self) -> Option<&ContextRecord> {
        if !self.search_results.is_empty() {
            let idx = self.search_state.selected().unwrap_or(0);
            self.search_results.get(idx).map(|r| &r.context)
        } else {
            let idx = self.memory_state.selected().unwrap_or(0);
            self.memories.get(idx).map(|m| &m.context)
        }
    }

    fn refresh_memories(&mut self, conn: &Connection) -> Result<()> {
        let proj = self.selected_project().map(|s| s.to_string());
        let cat = self.selected_category().map(|s| s.to_string());

        if cat.is_some() {
            // Filter by category using list_by_scope
            self.memories = context::list_by_scope(
                conn,
                Some("memory"),
                cat.as_deref(),
                proj.as_deref(),
                200,
            )?
            .into_iter()
            .map(|ctx| {
                let updated_at: chrono::DateTime<chrono::Utc> =
                    ctx.updated_at.parse().unwrap_or_else(|_| chrono::Utc::now());
                let score = rememora::hotness::final_score(ctx.importance, ctx.active_count, &updated_at);
                ScoredContext { context: ctx, score }
            })
            .collect();
            // Sort by score descending
            self.memories.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        } else {
            self.memories = hierarchy::get_l0_map(conn, proj.as_deref())?;
        }

        if self.memories.is_empty() {
            self.memory_state.select(None);
        } else {
            self.memory_state.select(Some(0));
        }
        self.detail_scroll = 0;
        Ok(())
    }

    fn do_search(&mut self, conn: &Connection) -> Result<()> {
        if self.search_query.trim().is_empty() {
            self.search_results.clear();
            self.search_state.select(None);
            return Ok(());
        }
        let proj = self.selected_project().map(|s| s.to_string());
        self.search_results =
            search::search(conn, &self.search_query, proj.as_deref(), None, 50)?;
        if self.search_results.is_empty() {
            self.search_state.select(None);
        } else {
            self.search_state.select(Some(0));
        }
        Ok(())
    }
}

// ── Main run ─────────────────────────────────────────────────────────────

pub fn run(conn: &Connection) -> Result<()> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = run_app(&mut terminal, conn);

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

fn run_app(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    conn: &Connection,
) -> Result<()> {
    let mut app = App::new(conn)?;

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        if app.quit {
            break;
        }

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                handle_key(&mut app, key.code, conn)?;
            }
        }
    }

    Ok(())
}

// ── Key handling ─────────────────────────────────────────────────────────

fn handle_key(app: &mut App, code: KeyCode, conn: &Connection) -> Result<()> {
    // Search mode input
    if app.search_active {
        match code {
            KeyCode::Esc => {
                app.search_active = false;
                app.search_query.clear();
                app.search_results.clear();
                app.search_state.select(None);
            }
            KeyCode::Backspace => {
                app.search_query.pop();
                app.do_search(conn)?;
            }
            KeyCode::Char(c) => {
                app.search_query.push(c);
                app.do_search(conn)?;
            }
            KeyCode::Down => {
                let len = app.search_results.len();
                if len > 0 {
                    let i = app.search_state.selected().unwrap_or(0);
                    app.search_state.select(Some((i + 1).min(len - 1)));
                    app.detail_scroll = 0;
                }
            }
            KeyCode::Up => {
                let len = app.search_results.len();
                if len > 0 {
                    let i = app.search_state.selected().unwrap_or(0);
                    app.search_state
                        .select(Some(i.saturating_sub(1)));
                    app.detail_scroll = 0;
                }
            }
            KeyCode::Enter if !app.search_results.is_empty() => {
                // Keep search results visible, exit input mode
                app.search_active = false;
                app.focus = Panel::Detail;
            }
            _ => {}
        }
        return Ok(());
    }

    // Normal mode
    match code {
        KeyCode::Char('q') => app.quit = true,
        KeyCode::Char('/') => {
            app.search_active = true;
            app.search_query.clear();
            app.search_results.clear();
            app.search_state.select(None);
        }
        KeyCode::Tab => {
            app.focus = app.focus.next();
        }
        KeyCode::BackTab => {
            app.focus = app.focus.prev();
        }
        KeyCode::Char('h') => {
            app.focus = app.focus.prev();
        }
        KeyCode::Char('l') => {
            app.focus = app.focus.next();
        }
        KeyCode::Esc if !app.search_results.is_empty() => {
            // Clear search results if any
            app.search_results.clear();
            app.search_state.select(None);
        }
        KeyCode::Char('j') | KeyCode::Down => match app.focus {
            Panel::Projects => {
                if app.showing_categories {
                    let len = app.categories.len();
                    if len > 0 {
                        let i = app.category_state.selected().unwrap_or(0);
                        app.category_state.select(Some((i + 1).min(len - 1)));
                        app.refresh_memories(conn)?;
                    }
                } else {
                    let len = app.projects.len();
                    if len > 0 {
                        let i = app.project_state.selected().unwrap_or(0);
                        app.project_state.select(Some((i + 1).min(len - 1)));
                        app.showing_categories = false;
                        app.category_state.select(Some(0));
                        app.refresh_memories(conn)?;
                    }
                }
            }
            Panel::Memories => {
                let len = if !app.search_results.is_empty() {
                    app.search_results.len()
                } else {
                    app.memories.len()
                };
                if len > 0 {
                    let state = if !app.search_results.is_empty() {
                        &mut app.search_state
                    } else {
                        &mut app.memory_state
                    };
                    let i = state.selected().unwrap_or(0);
                    state.select(Some((i + 1).min(len - 1)));
                    app.detail_scroll = 0;
                }
            }
            Panel::Detail => {
                app.detail_scroll = app.detail_scroll.saturating_add(1);
            }
        },
        KeyCode::Char('k') | KeyCode::Up => match app.focus {
            Panel::Projects => {
                if app.showing_categories {
                    let i = app.category_state.selected().unwrap_or(0);
                    if i == 0 {
                        // Go back to project list
                        app.showing_categories = false;
                        app.category_state.select(Some(0));
                        app.refresh_memories(conn)?;
                    } else {
                        app.category_state.select(Some(i - 1));
                        app.refresh_memories(conn)?;
                    }
                } else {
                    let i = app.project_state.selected().unwrap_or(0);
                    app.project_state.select(Some(i.saturating_sub(1)));
                    app.refresh_memories(conn)?;
                }
            }
            Panel::Memories => {
                let len = if !app.search_results.is_empty() {
                    app.search_results.len()
                } else {
                    app.memories.len()
                };
                if len > 0 {
                    let state = if !app.search_results.is_empty() {
                        &mut app.search_state
                    } else {
                        &mut app.memory_state
                    };
                    let i = state.selected().unwrap_or(0);
                    state.select(Some(i.saturating_sub(1)));
                    app.detail_scroll = 0;
                }
            }
            Panel::Detail => {
                app.detail_scroll = app.detail_scroll.saturating_sub(1);
            }
        },
        KeyCode::Enter => match app.focus {
            Panel::Projects => {
                if !app.showing_categories {
                    // Expand: show categories under selected project
                    app.showing_categories = true;
                    app.category_state.select(Some(0));
                    app.refresh_memories(conn)?;
                } else {
                    // Select category → move focus to memories
                    app.focus = Panel::Memories;
                }
            }
            Panel::Memories => {
                app.focus = Panel::Detail;
                app.detail_scroll = 0;
            }
            Panel::Detail => {
                // Noop in detail, already viewing
            }
        },
        _ => {}
    }

    Ok(())
}

// ── Drawing ──────────────────────────────────────────────────────────────

fn draw(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Top-level: main area + 1-line status bar
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(size);

    let main_area = outer[0];
    let status_area = outer[1];

    // Main layout: left sidebar | right content area
    let main_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(24), Constraint::Percentage(75)])
        .split(main_area);

    // Right side: memories list on top, detail on bottom
    let right_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(main_chunks[1]);

    draw_sidebar(f, app, main_chunks[0]);
    draw_memories(f, app, right_chunks[0]);
    draw_detail(f, app, right_chunks[1]);
    draw_status_bar(f, app, status_area);

    // Search overlay
    if app.search_active {
        draw_search(f, app, size);
    }
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Projects;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    if app.showing_categories {
        // Split sidebar: projects on top, categories on bottom
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(app.projects.len() as u16 + 2), Constraint::Min(3)])
            .split(area);

        // Projects (collapsed)
        let proj_items: Vec<ListItem> = app
            .projects
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let selected = app.project_state.selected() == Some(i);
                let marker = if selected && i > 0 { "v " } else if i > 0 { "> " } else { "  " };
                let style = if selected && !app.showing_categories {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(format!("{marker}{p}")).style(style)
            })
            .collect();

        let proj_block = Block::default()
            .title(" Projects ")
            .borders(Borders::ALL)
            .border_style(border_style);

        let proj_list = List::new(proj_items).block(proj_block);
        f.render_widget(proj_list, chunks[0]);

        // Categories
        let cat_items: Vec<ListItem> = app
            .categories
            .iter()
            .map(|c| {
                let style = Style::default().fg(Color::White);
                ListItem::new(format!("  {c}")).style(style)
            })
            .collect();

        let cat_border = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let cat_block = Block::default()
            .title(" Categories ")
            .borders(Borders::ALL)
            .border_style(cat_border);

        let cat_list = List::new(cat_items)
            .block(cat_block)
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        f.render_stateful_widget(cat_list, chunks[1], &mut app.category_state);
    } else {
        // Just projects
        let items: Vec<ListItem> = app
            .projects
            .iter()
            .map(|p| {
                let style = Style::default().fg(Color::White);
                ListItem::new(format!("  {p}")).style(style)
            })
            .collect();

        let block = Block::default()
            .title(" Projects ")
            .borders(Borders::ALL)
            .border_style(border_style);

        let list = List::new(items)
            .block(block)
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");

        f.render_stateful_widget(list, area, &mut app.project_state);
    }
}

fn draw_memories(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Memories;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let (items, count): (Vec<ListItem>, usize) = if !app.search_results.is_empty() {
        let items: Vec<ListItem> = app
            .search_results
            .iter()
            .map(|r| memory_list_item(&r.context, Some(r.rank)))
            .collect();
        let count = items.len();
        (items, count)
    } else {
        let items: Vec<ListItem> = app
            .memories
            .iter()
            .map(|m| memory_list_item(&m.context, Some(m.score)))
            .collect();
        let count = items.len();
        (items, count)
    };

    let title = if !app.search_results.is_empty() {
        format!(" Search Results ({count}) ")
    } else {
        format!(" Memories ({count}) ")
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let state = if !app.search_results.is_empty() {
        &mut app.search_state
    } else {
        &mut app.memory_state
    };

    f.render_stateful_widget(list, area, state);
}

fn memory_list_item<'a>(ctx: &ContextRecord, score: Option<f64>) -> ListItem<'a> {
    let cat_color = category_color(ctx.category.as_deref().unwrap_or(""));
    let cat_label = ctx.category.as_deref().unwrap_or("?");
    let score_str = score.map(|s| format!("{:.2}", s)).unwrap_or_default();

    // Truncate name to fit
    let name = if ctx.abstract_text.len() > 60 {
        format!("{}...", &ctx.abstract_text[..57])
    } else {
        ctx.abstract_text.clone()
    };

    let line = Line::from(vec![
        Span::styled(
            format!("[{cat_label}]"),
            Style::default().fg(cat_color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(name, Style::default().fg(Color::White)),
        Span::raw("  "),
        Span::styled(score_str, Style::default().fg(Color::DarkGray)),
    ]);

    ListItem::new(line)
}

fn draw_detail(f: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Panel::Detail;
    let border_style = if focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if let Some(ctx) = app.selected_memory() {
        let mut lines = Vec::new();

        // Header
        lines.push(Line::from(vec![
            Span::styled("Name: ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(&ctx.name),
        ]));
        lines.push(Line::from(vec![
            Span::styled("ID: ", Style::default().fg(Color::Yellow)),
            Span::styled(&ctx.id, Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("URI: ", Style::default().fg(Color::Yellow)),
            Span::styled(&ctx.uri, Style::default().fg(Color::DarkGray)),
        ]));
        if let Some(ref cat) = ctx.category {
            lines.push(Line::from(vec![
                Span::styled("Category: ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    cat.as_str(),
                    Style::default().fg(category_color(cat)),
                ),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("Importance: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{:.1}", ctx.importance)),
            Span::raw("  "),
            Span::styled("Accesses: ", Style::default().fg(Color::Yellow)),
            Span::raw(format!("{}", ctx.active_count)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Created: ", Style::default().fg(Color::Yellow)),
            Span::styled(format_datetime(&ctx.created_at), Style::default().fg(Color::DarkGray)),
            Span::raw("  "),
            Span::styled("Updated: ", Style::default().fg(Color::Yellow)),
            Span::styled(format_datetime(&ctx.updated_at), Style::default().fg(Color::DarkGray)),
        ]));
        if let Some(ref agent) = ctx.source_agent {
            lines.push(Line::from(vec![
                Span::styled("Agent: ", Style::default().fg(Color::Yellow)),
                Span::raw(agent.as_str()),
            ]));
        }

        lines.push(Line::raw(""));

        // L0 Abstract
        lines.push(Line::from(Span::styled(
            "--- L0 Abstract ---",
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )));
        for line in ctx.abstract_text.lines() {
            lines.push(Line::raw(line.to_string()));
        }

        // L1 Overview
        if !ctx.overview.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "--- L1 Overview ---",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )));
            for line in ctx.overview.lines() {
                lines.push(Line::raw(line.to_string()));
            }
        }

        // L2 Content
        if !ctx.content.is_empty() {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "--- L2 Content ---",
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )));
            for line in ctx.content.lines() {
                lines.push(Line::raw(line.to_string()));
            }
        }

        // Tags
        if !ctx.tags.is_empty() && ctx.tags != "[]" {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled("Tags: ", Style::default().fg(Color::Yellow)),
                Span::styled(&ctx.tags, Style::default().fg(Color::DarkGray)),
            ]));
        }

        let text = Text::from(lines);
        let para = Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0));

        f.render_widget(para, area);
    } else {
        let text = Paragraph::new("No memory selected")
            .block(block)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(text, area);
    }
}

fn draw_search(f: &mut Frame, app: &mut App, area: Rect) {
    // Search bar at the bottom
    let search_height = 3;
    let search_area = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(search_height),
        width: area.width,
        height: search_height.min(area.height),
    };

    f.render_widget(Clear, search_area);

    let block = Block::default()
        .title(" Search (Esc to close) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let search_text = format!("/ {}", app.search_query);
    let cursor_text = format!("{search_text}_");

    let para = Paragraph::new(cursor_text)
        .block(block)
        .style(Style::default().fg(Color::White));

    f.render_widget(para, search_area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let hints = if app.search_active {
        " Esc: close search | Up/Down: navigate results | Enter: select "
    } else {
        " j/k: navigate | h/l: switch panel | Tab: cycle | Enter: expand | /: search | q: quit "
    };

    let focus_label = match app.focus {
        Panel::Projects => "Projects",
        Panel::Memories => "Memories",
        Panel::Detail => "Detail",
    };

    let bar = Line::from(vec![
        Span::styled(
            format!(" {focus_label} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(hints, Style::default().fg(Color::DarkGray)),
    ]);

    f.render_widget(Paragraph::new(bar), area);
}

// ── Helpers ──────────────────────────────────────────────────────────────

fn category_color(category: &str) -> Color {
    match category {
        "preference" => Color::Green,
        "entity" => Color::Blue,
        "decision" => Color::Yellow,
        "event" => Color::Magenta,
        "case" => Color::Red,
        "pattern" => Color::Cyan,
        _ => Color::White,
    }
}

fn format_datetime(s: &str) -> String {
    // Parse RFC3339 and show a short form
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        dt.format("%Y-%m-%d %H:%M").to_string()
    } else {
        s.to_string()
    }
}
