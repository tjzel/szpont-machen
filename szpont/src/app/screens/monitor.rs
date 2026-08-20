use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState, Wrap,
};

use crate::app::{App, Pending, Screen};
use crate::core::snapshot::SessionRow;
use crate::core::{Liveness, format_age, format_tokens, now_ms, truncate};

const POPUP_WIDTH: u16 = 72;
const PICKER_WIDTH: u16 = 44;
const MIN_FLEX_WIDTH: u16 = 12;
const PROJECT_WIDTH: u16 = 14;
const FLAT_FIXED_COLUMNS: [u16; 7] = [5, 1, 7, 7, PROJECT_WIDTH, 9, 20];
const ARCHIVE_EXTRA_COLUMN: u16 = 16;
const TREE_FIXED_COLUMNS: [u16; 6] = [1, 7, 7, 9, 20, 5];

fn split_flex_width(area_width: u16, archive: bool, longest_title: usize) -> (usize, usize) {
    let fixed: u16 =
        FLAT_FIXED_COLUMNS.iter().sum::<u16>() + if archive { ARCHIVE_EXTRA_COLUMN } else { 0 };
    let columns = FLAT_FIXED_COLUMNS.len() as u16 + 2 + u16::from(archive);
    let flex = area_width.saturating_sub(fixed + columns.saturating_sub(1));
    let share = (flex / 2).max(MIN_FLEX_WIDTH);
    let wanted = u16::try_from(longest_title).unwrap_or(u16::MAX);
    let title = wanted.clamp(MIN_FLEX_WIDTH.min(share), share);
    let repo = flex.saturating_sub(title).max(MIN_FLEX_WIDTH);
    (title as usize, repo as usize)
}

fn row_title(row: &SessionRow) -> &str {
    row.session
        .title
        .as_deref()
        .or(row.session.preview.as_deref())
        .unwrap_or("-")
}

fn fixed_table_width(columns: &[u16], extra: Option<u16>) -> u16 {
    let base: u16 = columns.iter().sum();
    let count = columns.len() as u16 + u16::from(extra.is_some());
    base + extra.unwrap_or(0) + count
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let tall = frame.area().height >= 22 && frame.area().width >= 90;
    let width = frame.area().width.max(1) as usize;
    let cap = footer_cap(frame.area().height);
    let footer = footer_line(app, width, cap);
    let footer_height = wrapped_height(&footer, width, cap);
    let status_height = wrapped_height(&header_line(app, true), width, MAX_HEADER_LINES);
    let limits_height = wrapped_height(&limits_line(app), width, MAX_HEADER_LINES);
    let header_height = if tall {
        crate::app::widgets::logo::HEIGHT
    } else {
        status_height + limits_height
    };
    let [header_area, table_area, separator_area, footer_area] = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(footer_height),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new("─".repeat(frame.area().width as usize))
            .style(Style::new().fg(Color::DarkGray)),
        separator_area,
    );
    if tall {
        let [logo_area, info_area] = Layout::horizontal([
            Constraint::Length(crate::app::widgets::logo::WIDTH),
            Constraint::Min(0),
        ])
        .areas(header_area);
        let flapped = app.scanning && (now_ms() / 300) % 2 == 0;
        frame.render_widget(
            Paragraph::new(crate::app::widgets::logo::carton(flapped)),
            logo_area,
        );
        let [title_line, info_lines, limits_area, scan_line, _] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(info_area);
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" szpont machen", Style::new().fg(Color::Yellow).bold()),
                Span::styled(
                    format!(" v{}", env!("CARGO_PKG_VERSION")),
                    Style::new().fg(Color::DarkGray),
                ),
            ])),
            title_line,
        );
        frame.render_widget(
            Paragraph::new(header_line(app, false)).wrap(Wrap { trim: true }),
            info_lines,
        );
        frame.render_widget(
            Paragraph::new(limits_line(app)).wrap(Wrap { trim: true }),
            limits_area,
        );
        if let Some(scan) = &app.scan_status {
            frame.render_widget(
                Paragraph::new(format!(" {scan}")).style(Style::new().fg(Color::DarkGray)),
                scan_line,
            );
        }
    } else {
        let [status_area, limits_area] = Layout::vertical([
            Constraint::Length(status_height),
            Constraint::Length(limits_height),
        ])
        .areas(header_area);
        frame.render_widget(
            Paragraph::new(header_line(app, true)).wrap(Wrap { trim: true }),
            status_area,
        );
        frame.render_widget(
            Paragraph::new(limits_line(app)).wrap(Wrap { trim: true }),
            limits_area,
        );
    }
    draw_table(frame, app, table_area);
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: true }),
        footer_area,
    );
    if let Some(Pending::NewSession { tools, selected }) = &app.pending {
        draw_new_session_popup(frame, app, tools, *selected);
    }
    if app.show_limits {
        draw_limits_popup(frame, app);
    }
    if app.show_detail {
        draw_detail_popup(frame, app);
    }
}

fn draw_detail_popup(frame: &mut Frame, app: &App) {
    let Some(row) = app.selected_row() else {
        return;
    };
    let now = now_ms();
    let dim = Style::new().fg(Color::DarkGray);
    let mut lines = vec![
        Line::from(vec![
            Span::styled("tool     ", dim),
            Span::raw(row.session.key.tool.display_name().to_string()),
        ]),
        Line::from(vec![
            Span::styled("session  ", dim),
            Span::raw(row.session.key.id.clone()),
        ]),
        Line::from(vec![
            Span::styled("title    ", dim),
            Span::raw(row.session.title.clone().unwrap_or_else(|| "-".to_string())),
        ]),
        Line::from(vec![
            Span::styled("cwd      ", dim),
            Span::raw(row.project_label().unwrap_or_else(|| {
                row.session
                    .cwd
                    .as_deref()
                    .map_or_else(|| "?".to_string(), |p| p.display().to_string())
            })),
        ]),
        Line::from(vec![
            Span::styled("model    ", dim),
            Span::raw(row.session.model.clone().unwrap_or_else(|| "-".to_string())),
        ]),
        Line::from(vec![
            Span::styled("updated  ", dim),
            Span::raw(format!(
                "{} ago",
                format_age(now - row.session.updated_at_ms)
            )),
        ]),
    ];
    if let Some(tokens) = row.context_tokens.filter(|t| *t > 0) {
        let text = match row.context_window {
            Some(window) if window > 0 => format!(
                "{} of {} ({:.0}%)",
                format_tokens(tokens),
                format_tokens(window),
                (tokens as f64 / window as f64 * 100.0).min(100.0)
            ),
            _ => format!("{} (window unknown)", format_tokens(tokens)),
        };
        lines.push(Line::from(vec![
            Span::styled("context  ", dim),
            Span::raw(text),
        ]));
    }
    if let Some(usage) = &row.usage {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("token usage", Style::new().bold())));
        for (label, value) in [
            ("input", usage.input_uncached),
            ("cache read", usage.input_cache_read),
            ("cache write", usage.input_cache_write),
            ("output", usage.output),
        ] {
            lines.push(Line::from(vec![
                Span::styled(format!("  {label:<12}"), dim),
                Span::raw(format_tokens(value)),
            ]));
        }
        lines.push(Line::from(vec![
            Span::styled("  total       ".to_string(), dim),
            Span::styled(format_tokens(usage.total()), Style::new().bold()),
        ]));
    } else if let Some(tokens) = row.session.native_tokens_used {
        lines.push(Line::from(vec![
            Span::styled("tokens   ", dim),
            Span::raw(format!("{} (reported by the tool)", format_tokens(tokens))),
        ]));
    }
    if let Some(preview) = &row.session.preview {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("last prompt", Style::new().bold())));
        lines.push(Line::from(format!("  {}", truncate(preview, 66))));
    }
    let area = popup_area(frame.area(), POPUP_WIDTH, lines.len());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" session ")
                .border_style(Style::new().fg(Color::Blue)),
        ),
        area,
    );
}

fn popup_area(screen: Rect, width: u16, content_lines: usize) -> Rect {
    let height = (content_lines as u16 + 2).min(screen.height.saturating_sub(2));
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(screen);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn draw_limits_popup(frame: &mut Frame, app: &App) {
    let now = now_ms();
    let mut lines: Vec<Line> = Vec::new();
    if app.limits_disabled {
        lines.push(Line::from("limit data is hidden in promo mode"));
    } else if app.limits.is_empty() {
        lines.push(Line::from("no limit data yet"));
    }
    for tool in &app.limits {
        let mut header = vec![Span::styled(
            tool.tool.display_name().to_string(),
            Style::new().bold(),
        )];
        if let Some(plan) = &tool.plan {
            header.push(Span::styled(
                format!("  ({plan} plan)"),
                Style::new().fg(Color::DarkGray),
            ));
        }
        header.push(Span::styled(
            format!("  via {}", tool.source),
            Style::new().fg(Color::DarkGray),
        ));
        lines.push(Line::from(header));
        if tool.windows.is_empty() {
            lines.push(Line::from(Span::styled(
                "  no limit data",
                Style::new().fg(Color::DarkGray),
            )));
        }
        for window in &tool.windows {
            let mut spans = vec![Span::raw(format!("  {:<5}", window.label))];
            match window.used_percent {
                Some(pct) => {
                    let color = usage_color(pct);
                    spans.push(Span::styled(usage_bar(pct, 24), Style::new().fg(color)));
                    let marker = if window.estimated { " est" } else { "" };
                    spans.push(Span::styled(
                        format!(" {pct:.0}%{marker}"),
                        Style::new().fg(color),
                    ));
                }
                None => {
                    spans.push(Span::styled(
                        "no ceiling observed yet".to_string(),
                        Style::new().fg(Color::DarkGray),
                    ));
                }
            }
            if let Some(tokens) = window.tokens {
                spans.push(Span::styled(
                    format!("  {} tokens", format_tokens(tokens)),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            if let Some(resets_at) = window.resets_at {
                let remaining_ms = resets_at.saturating_mul(1000).saturating_sub(now);
                if remaining_ms > 0 {
                    spans.push(Span::styled(
                        format!("  ↻ in {}", format_age(remaining_ms)),
                        Style::new().fg(Color::Cyan),
                    ));
                }
            }
            lines.push(Line::from(spans));
        }
        if let Some(note) = &tool.note {
            let style = if tool.stale_credentials {
                Style::new().fg(Color::Yellow)
            } else {
                Style::new().fg(Color::DarkGray)
            };
            lines.push(Line::from(Span::styled(format!("  {note}"), style)));
        }
        lines.push(Line::from(""));
    }
    let area = popup_area(frame.area(), POPUP_WIDTH, lines.len());
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::bordered()
                .title(" limit usage ")
                .border_style(Style::new().fg(Color::Blue)),
        ),
        area,
    );
}

fn draw_new_session_popup(
    frame: &mut Frame,
    app: &App,
    tools: &[crate::core::ToolId],
    selected: usize,
) {
    let area = popup_area(frame.area(), PICKER_WIDTH, tools.len());
    frame.render_widget(Clear, area);
    let where_label = app
        .repo
        .as_ref()
        .filter(|_| app.screen == Screen::Repo)
        .map_or_else(|| "current directory".to_string(), |ctx| ctx.name.clone());
    let items: Vec<ListItem> = tools
        .iter()
        .enumerate()
        .map(|(i, tool)| {
            let marker = if i == selected { "▸" } else { " " };
            let mut style = Style::new().fg(tool_color(*tool));
            if i == selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            ListItem::new(format!(" {marker} {}. {}", i + 1, tool.display_name())).style(style)
        })
        .collect();
    let list = List::new(items).block(
        Block::bordered()
            .title(format!(" new session in {} ", truncate(&where_label, 32)))
            .border_style(Style::new().fg(Color::Green)),
    );
    frame.render_widget(list, area);
}

fn header_line(app: &App, with_logo: bool) -> Line<'static> {
    let visible = app.visible_indices();
    let mut running = 0;
    let mut waiting = 0;
    let mut open = 0;
    for &i in &visible {
        match app.rows[i].liveness {
            Liveness::Running => running += 1,
            Liveness::WaitingForInput => waiting += 1,
            Liveness::Open => open += 1,
            Liveness::Idle => {}
        }
    }
    let scope = if app.allowlist_mode {
        match app.screen {
            Screen::Archive => "promo archive".to_string(),
            Screen::Tree if app.tree_of_archive() => "promo archive by project".to_string(),
            Screen::Tree => "promo by project".to_string(),
            _ => "promo allowlist".to_string(),
        }
    } else {
        match (app.screen, &app.repo) {
            (Screen::Archive, _) => "archive".to_string(),
            (Screen::Tree, _) if app.tree_of_archive() => "archive by location".to_string(),
            (Screen::Tree, _) => "by location".to_string(),
            (Screen::Repo, _) if app.scope_project.is_some() => {
                format!("project: {}", app.scope_project.clone().unwrap_or_default())
            }
            (Screen::Repo, Some(ctx)) if ctx.is_git => format!("repo: {}", ctx.name),
            (Screen::Repo, Some(ctx)) => format!("dir: {}", ctx.name),
            _ => "all repos".to_string(),
        }
    };
    let mut spans = Vec::new();
    if with_logo {
        spans.push(crate::app::widgets::logo::compact());
        spans.push(Span::styled(
            format!(" v{}", env!("CARGO_PKG_VERSION")),
            Style::new().fg(Color::DarkGray),
        ));
        spans.push(Span::raw("  "));
    } else {
        spans.push(Span::raw(" "));
    }
    spans.extend([
        Span::styled(
            scope,
            Style::new()
                .fg(app
                    .scope_project
                    .as_deref()
                    .map_or(Color::Magenta, project_color))
                .bold(),
        ),
        Span::raw("   "),
        Span::styled(format!("{running} RUNNING"), Style::new().fg(Color::Green)),
        Span::raw("   "),
        Span::styled(format!("{waiting} BLOCKED"), Style::new().fg(Color::Yellow)),
        Span::raw("   "),
        Span::styled(format!("{open} IDLE"), Style::new().fg(Color::Cyan)),
        Span::raw("   "),
        Span::styled(
            format!("{} sessions", visible.len()),
            Style::new().fg(Color::DarkGray),
        ),
    ]);
    if app.pending_snapshot.is_some() {
        let bright_phase = (now_ms() / 600) % 2 == 0;
        let style = if bright_phase {
            Style::new().fg(Color::Yellow).bold()
        } else {
            Style::new().fg(Color::Yellow).dim()
        };
        spans.push(Span::styled(
            "   ⟳ list changed — press r to refresh",
            style,
        ));
    }
    if let Some(tool) = app.tool_filter {
        spans.push(Span::styled(
            format!("   [tool: {}]", tool.as_str()),
            Style::new().fg(Color::Blue).bold(),
        ));
    }
    if let Some(filter) = &app.filter {
        spans.push(Span::styled(
            format!("   [filter: {filter}]"),
            Style::new().fg(Color::Blue).bold(),
        ));
    }
    if with_logo && let Some(scan) = &app.scan_status {
        spans.push(Span::styled(
            format!("   ⋯ {scan}"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    Line::from(spans)
}

fn limits_line(app: &App) -> Line<'static> {
    let mut spans = vec![Span::styled(
        " limits",
        Style::new().fg(Color::DarkGray).bold(),
    )];
    spans.extend(limits_spans(app));
    Line::from(spans)
}

fn limits_spans(app: &App) -> Vec<Span<'static>> {
    if app.limits_disabled {
        return vec![Span::styled(": hidden", Style::new().fg(Color::DarkGray))];
    }
    if app.limits.is_empty() {
        return vec![Span::styled(
            ": measuring…",
            Style::new().fg(Color::DarkGray),
        )];
    }
    let now = now_ms();
    let mut spans = vec![Span::raw("  ")];
    let mut first = true;
    for tool in &app.limits {
        if !first {
            spans.push(Span::styled("  │  ", Style::new().fg(Color::DarkGray)));
        }
        first = false;
        spans.push(Span::styled(
            format!("{} ", tool.tool.as_str()),
            Style::new().fg(Color::DarkGray).bold(),
        ));
        if tool.stale_credentials {
            spans.push(Span::styled(
                "⚠ sign-in expired",
                Style::new().fg(Color::Yellow).bold(),
            ));
            if tool.windows.is_empty() {
                continue;
            }
            spans.push(Span::raw("  "));
        } else if tool.windows.is_empty() {
            spans.push(Span::styled("—", Style::new().fg(Color::DarkGray)));
            continue;
        }
        for (i, window) in tool.windows.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            match window.used_percent {
                Some(pct) => {
                    let color = usage_color(pct);
                    let marker = if window.estimated { "~" } else { "" };
                    spans.push(Span::raw(format!("{} ", window.label)));
                    spans.push(Span::styled(usage_bar(pct, 7), Style::new().fg(color)));
                    spans.push(Span::styled(
                        format!(" {pct:.0}%{marker}"),
                        Style::new().fg(color),
                    ));
                }
                None => {
                    spans.push(Span::raw(format!(
                        "{} {}",
                        window.label,
                        window.tokens.map_or_else(|| "?".to_string(), format_tokens)
                    )));
                }
            }
            if let Some(remaining) = reset_in(window, now) {
                spans.push(Span::styled(
                    format!(" ↻{remaining}"),
                    Style::new().fg(Color::DarkGray),
                ));
            }
        }
    }
    spans
}

fn reset_in(window: &crate::limits::WindowUsage, now: i64) -> Option<String> {
    let remaining_ms = window
        .resets_at?
        .saturating_mul(1000)
        .saturating_sub(now)
        .max(0);
    if remaining_ms == 0 {
        return None;
    }
    Some(format_age(remaining_ms))
}

fn usage_color(pct: f64) -> Color {
    if pct >= 90.0 {
        Color::Red
    } else if pct >= 70.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn usage_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64)
        .round()
        .clamp(0.0, width as f64) as usize;
    format!("{}{}", "▓".repeat(filled), "░".repeat(width - filled))
}

fn draw_tree_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let visible = app.visible_indices();
    if visible.is_empty() {
        let message = if app.received_first_snapshot {
            "no active sessions — start claude, codex or kimi in a repo"
        } else {
            "scanning sessions…"
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::new().fg(Color::DarkGray))
                .centered(),
            area,
        );
        return;
    }
    let now = now_ms();
    let title_width = area
        .width
        .saturating_sub(fixed_table_width(&TREE_FIXED_COLUMNS, None))
        .max(10) as usize;
    let mut roots = TreeDir::new(String::new());
    for &i in &visible {
        let components = crate::app::row_tree_components(&app.rows[i]);
        roots.insert(&components, i);
    }
    let mut table_rows: Vec<Row> = Vec::new();
    let mut selected_render = 0usize;
    let mut session_position = 0usize;
    let root_count = roots.children.len();
    for (index, root) in roots.children.iter().enumerate() {
        render_tree_dir(
            app,
            root,
            "",
            index + 1 == root_count,
            true,
            now,
            title_width,
            &mut table_rows,
            &mut session_position,
            &mut selected_render,
        );
    }
    let table = Table::new(
        table_rows,
        [
            Constraint::Length(TREE_FIXED_COLUMNS[0]),
            Constraint::Min(24),
            Constraint::Length(TREE_FIXED_COLUMNS[1]),
            Constraint::Length(TREE_FIXED_COLUMNS[2]),
            Constraint::Length(TREE_FIXED_COLUMNS[3]),
            Constraint::Length(TREE_FIXED_COLUMNS[4]),
            Constraint::Length(TREE_FIXED_COLUMNS[5]),
        ],
    )
    .header(
        Row::new([
            "",
            "LOCATION / TITLE",
            "STATUS",
            "TOOL",
            "TOKENS",
            "CTX",
            "LAST",
        ])
        .style(
            Style::new()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = TableState::default()
        .with_offset(app.scroll)
        .with_selected(Some(selected_render));
    frame.render_stateful_widget(table, area, &mut state);
    app.scroll = state.offset();
}

struct TreeDir {
    name: String,
    children: Vec<TreeDir>,
    sessions: Vec<usize>,
}

impl TreeDir {
    fn new(name: String) -> TreeDir {
        TreeDir {
            name,
            children: Vec::new(),
            sessions: Vec::new(),
        }
    }

    fn insert(&mut self, components: &[String], row: usize) {
        let Some(first) = components.first() else {
            self.sessions.push(row);
            return;
        };
        let position = self.children.iter().position(|c| c.name == *first);
        let child = if let Some(pos) = position {
            &mut self.children[pos]
        } else {
            self.children.push(TreeDir::new(first.clone()));
            self.children.last_mut().unwrap()
        };
        child.insert(&components[1..], row);
    }
}

#[allow(clippy::too_many_arguments)]
fn render_tree_dir(
    app: &App,
    dir: &TreeDir,
    prefix: &str,
    is_last: bool,
    is_root: bool,
    now: i64,
    title_width: usize,
    table_rows: &mut Vec<Row<'static>>,
    session_position: &mut usize,
    selected_render: &mut usize,
) {
    let branch = if is_root {
        ""
    } else if is_last {
        "└─ "
    } else {
        "├─ "
    };
    table_rows.push(
        Row::new(vec![
            Cell::from(""),
            Cell::from(format!("{prefix}{branch}{}", dir.name)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ])
        .style(Style::new().fg(Color::Blue).bold()),
    );
    let child_prefix = if is_root {
        String::new()
    } else {
        format!("{prefix}{}", if is_last { "   " } else { "│  " })
    };
    let total = dir.sessions.len() + dir.children.len();
    for (position, &row_index) in dir.sessions.iter().enumerate() {
        let item_last = position + 1 == total;
        let item_branch = if item_last { "└─ " } else { "├─ " };
        if *session_position == app.selected {
            *selected_render = table_rows.len();
        }
        *session_position += 1;
        let row = &app.rows[row_index];
        let (status_label, status_style) = liveness_label(row.liveness);
        let mark = if app.marked.contains(&row.session.key) {
            "✓"
        } else {
            " "
        };
        let title = row
            .session
            .title
            .as_deref()
            .or(row.session.preview.as_deref())
            .unwrap_or("-");
        let label_width = title_width.saturating_sub(child_prefix.chars().count() + 3);
        table_rows.push(
            Row::new(vec![
                Cell::from(mark).style(Style::new().fg(Color::Magenta).bold()),
                Cell::from(Line::from(vec![
                    Span::styled(
                        format!("{child_prefix}{item_branch}"),
                        Style::new().fg(Color::DarkGray),
                    ),
                    Span::raw(truncate(title, label_width)),
                ])),
                Cell::from(status_label).style(status_style),
                Cell::from(row.session.key.tool.as_str())
                    .style(Style::new().fg(tool_color(row.session.key.tool))),
                Cell::from(
                    row.tokens_total()
                        .map_or_else(|| "-".to_string(), format_tokens),
                ),
                context_cell(row),
                Cell::from(format_age(now - row.session.updated_at_ms)),
            ])
            .style(match row.liveness {
                Liveness::Idle => Style::new().fg(Color::Gray),
                Liveness::WaitingForInput => Style::new().add_modifier(Modifier::BOLD),
                _ => Style::new(),
            }),
        );
    }
    let child_count = dir.children.len();
    for (position, child) in dir.children.iter().enumerate() {
        render_tree_dir(
            app,
            child,
            &child_prefix,
            position + 1 == child_count,
            false,
            now,
            title_width,
            table_rows,
            session_position,
            selected_render,
        );
    }
}

fn frame_style(app: &App) -> (Color, &'static str) {
    if let Some(mode) = app.marking {
        return match mode {
            crate::app::Marking::Complete => (Color::Yellow, " marking as completed "),
            crate::app::Marking::Delete => (Color::Red, " marking for deletion "),
        };
    }
    if matches!(app.pending, Some(Pending::NewSession { .. })) {
        return (Color::Green, " new session ");
    }
    match app.screen {
        Screen::Archive => (Color::Magenta, " archive "),
        Screen::Tree if app.tree_of_archive() => (Color::Magenta, " archive by location "),
        Screen::Tree => (Color::Blue, " sessions by location "),
        _ => (Color::DarkGray, " sessions "),
    }
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let (frame_color, frame_title) = frame_style(app);
    let block = Block::bordered()
        .border_style(Style::new().fg(frame_color))
        .title(frame_title)
        .title_style(Style::new().fg(frame_color).bold());
    let area = {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    };
    if app.screen == Screen::Tree {
        draw_tree_table(frame, app, area);
        return;
    }
    let visible = app.visible_indices();
    if visible.is_empty() {
        let message = if !app.received_first_snapshot {
            "scanning sessions…"
        } else if app.screen == Screen::Archive {
            "archive is empty — mark sessions as completed with c"
        } else {
            "no active sessions — start claude, codex or kimi in a repo"
        };
        frame.render_widget(
            Paragraph::new(message)
                .style(Style::new().fg(Color::DarkGray))
                .centered(),
            area,
        );
        return;
    }
    let now = now_ms();
    let archive = app.screen == Screen::Archive;
    let longest_title = visible
        .iter()
        .map(|&i| row_title(&app.rows[i]).chars().count())
        .max()
        .unwrap_or(0);
    let (title_width, repo_width) = split_flex_width(area.width, archive, longest_title);
    let rows: Vec<Row> = visible
        .iter()
        .map(|&i| {
            let row = &app.rows[i];
            session_row(
                row,
                now,
                archive,
                app.marked.contains(&row.session.key),
                title_width,
                repo_width,
            )
        })
        .collect();
    let mut widths = vec![
        Constraint::Length(FLAT_FIXED_COLUMNS[0]),
        Constraint::Length(FLAT_FIXED_COLUMNS[1]),
        Constraint::Length(FLAT_FIXED_COLUMNS[2]),
        Constraint::Length(FLAT_FIXED_COLUMNS[3]),
        Constraint::Length(FLAT_FIXED_COLUMNS[4]),
        Constraint::Length(title_width as u16),
        Constraint::Length(repo_width as u16),
        Constraint::Length(FLAT_FIXED_COLUMNS[5]),
        Constraint::Length(FLAT_FIXED_COLUMNS[6]),
    ];
    let mut header = vec![
        "LAST", "", "STATUS", "TOOL", "PROJECT", "TITLE", "REPO", "TOKENS", "CTX",
    ];
    if archive {
        widths.push(Constraint::Length(ARCHIVE_EXTRA_COLUMN));
        header.push("COMPLETED");
    }
    let table = Table::new(rows, widths)
        .header(
            Row::new(header).style(
                Style::new()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
        )
        .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    let mut state = TableState::default()
        .with_offset(app.scroll)
        .with_selected(Some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);
    app.scroll = state.offset();
}

fn session_row(
    row: &SessionRow,
    now: i64,
    archive: bool,
    marked: bool,
    title_width: usize,
    repo_width: usize,
) -> Row<'static> {
    let (status_label, status_style) = if archive {
        ("", Style::new())
    } else {
        liveness_label(row.liveness)
    };
    let title = row_title(row);
    let base = if archive {
        Style::new()
    } else {
        match row.liveness {
            Liveness::Idle => Style::new().fg(Color::Gray),
            Liveness::WaitingForInput => Style::new().add_modifier(Modifier::BOLD),
            _ => Style::new(),
        }
    };
    let mark = if marked { "✓" } else { " " };
    let mut cells = vec![
        Cell::from(format_age(now - row.session.updated_at_ms)),
        Cell::from(mark).style(Style::new().fg(Color::Magenta).bold()),
        Cell::from(status_label).style(status_style),
        Cell::from(row.session.key.tool.as_str())
            .style(Style::new().fg(tool_color(row.session.key.tool))),
        project_cell(row.project.as_deref()),
        Cell::from(truncate(title, title_width)),
        Cell::from(repo_label(row, repo_width)),
        Cell::from(
            row.tokens_total()
                .map_or_else(|| "-".to_string(), format_tokens),
        ),
        context_cell(row),
    ];
    if archive {
        let completed = match (row.completed_at, row.session.native_archived) {
            (Some(at), _) => format!("{} ago", format_age(now - at)),
            (None, true) => format!("[{}]", row.session.key.tool.as_str()),
            (None, false) => "-".to_string(),
        };
        cells.push(Cell::from(completed));
    }
    Row::new(cells).style(base)
}

fn project_cell(project: Option<&str>) -> Cell<'static> {
    match project.filter(|name| !name.is_empty()) {
        Some(name) => Cell::from(truncate(name, PROJECT_WIDTH as usize))
            .style(Style::new().fg(project_color(name))),
        None => Cell::from(""),
    }
}

fn project_color(name: &str) -> Color {
    const PALETTE: [u8; 10] = [39, 42, 214, 170, 45, 178, 204, 76, 110, 173];
    let mut hash: u32 = 2_166_136_261;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    Color::Indexed(PALETTE[(hash % 10) as usize])
}

fn tool_color(tool: crate::core::ToolId) -> Color {
    match tool {
        crate::core::ToolId::Claude => Color::LightRed,
        crate::core::ToolId::Codex => Color::LightCyan,
        crate::core::ToolId::Kimi => Color::LightMagenta,
    }
}

fn liveness_label(liveness: Liveness) -> (&'static str, Style) {
    match liveness {
        Liveness::Running => ("RUNNING", Style::new().fg(Color::Green).bold()),
        Liveness::WaitingForInput => ("BLOCKED", Style::new().fg(Color::Yellow).bold()),
        Liveness::Open => ("IDLE", Style::new().fg(Color::Cyan)),
        Liveness::Idle => ("", Style::new()),
    }
}

pub(super) fn footer_cap(area_height: u16) -> u16 {
    (area_height / 2).max(1)
}
const MAX_HEADER_LINES: u16 = 2;

pub(super) fn wrapped_height(line: &Line, area_width: usize, cap: u16) -> u16 {
    wrapped_rows(&line_text(line), area_width).min(cap)
}

fn line_text(line: &Line) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn text_width(text: &str) -> usize {
    Span::raw(text).width()
}

fn wrapped_rows(text: &str, area_width: usize) -> u16 {
    let width = area_width.max(1);
    let mut rows: u16 = 1;
    let mut used = 0usize;
    let mut gap = 0usize;
    for token in text.split_inclusive(char::is_whitespace) {
        let word = token.trim_end_matches(char::is_whitespace);
        let spacing = text_width(&token[word.len()..]);
        if !word.is_empty() {
            let wanted = text_width(word);
            if used == 0 {
                used = wanted;
            } else if used + gap + wanted <= width {
                used += gap + wanted;
            } else {
                rows = rows.saturating_add(1);
                used = wanted;
            }
            gap = 0;
            while used > width {
                rows = rows.saturating_add(1);
                used -= width;
            }
        }
        if used > 0 {
            gap += spacing;
        }
    }
    rows
}

pub(super) fn footer_line(app: &App, width: usize, cap: u16) -> Line<'static> {
    let status_style = Style::new().fg(Color::Yellow);
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    if let Some(input) = &app.filter_input {
        let matches = app.visible_indices().len();
        spans.push(Span::styled("find: ", status_style));
        spans.extend(input.cursor_spans(status_style));
        spans.push(Span::styled(
            format!("   {matches} match(es)   "),
            Style::new().fg(Color::DarkGray),
        ));
    } else if let Some(input) = &app.window_input {
        spans.push(Span::styled(
            format!("context window for {}: ", input.model),
            status_style,
        ));
        spans.extend(input.editor.cursor_spans(status_style));
        spans.push(Span::raw("   "));
    } else if let Some(input) = &app.rename_input {
        spans.push(Span::styled("rename: ", status_style));
        spans.extend(input.editor.cursor_spans(status_style));
        spans.push(Span::raw("   "));
    } else if let Some(input) = &app.project_input {
        spans.push(Span::styled(
            format!(
                "project for {}: ",
                crate::core::truncate(&input.dir.display().to_string(), 40)
            ),
            status_style,
        ));
        spans.extend(input.editor.cursor_spans(status_style));
        spans.push(Span::raw("   "));
    }
    let owned = footer_hints(app);
    let borrowed: Vec<(&str, &str)> = owned
        .iter()
        .map(|(key, label)| (key.as_str(), label.as_str()))
        .collect();
    let hints = hint_spans(&borrowed);
    if shows_status(app) {
        let fitted = app
            .status
            .as_deref()
            .and_then(|status| fit_status(status, &hints, width, cap));
        if let Some(status) = fitted {
            spans.push(Span::styled(status, status_style));
            spans.push(Span::styled(
                STATUS_SEPARATOR,
                Style::new().fg(Color::DarkGray),
            ));
        }
    }
    spans.extend(hints);
    Line::from(spans)
}

fn shows_status(app: &App) -> bool {
    app.status.is_some()
        && !app.show_limits
        && !app.show_detail
        && app.filter_input.is_none()
        && app.window_input.is_none()
        && app.rename_input.is_none()
        && app.project_input.is_none()
        && app.marking.is_none()
        && app.pending.is_none()
}

pub(super) fn footer_hints(app: &App) -> Vec<(String, String)> {
    fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, label)| ((*key).to_string(), (*label).to_string()))
            .collect()
    }
    let mut hints = if app.show_limits || app.show_detail {
        owned(&[("any key", "close")])
    } else if app.filter_input.is_some() {
        owned(&[("enter", "keep"), ("esc", "cancel")])
    } else if app.window_input.is_some()
        || app.rename_input.is_some()
        || app.project_input.is_some()
    {
        owned(&[("enter", "save"), ("esc", "cancel")])
    } else if let Some(mode) = app.marking {
        let (toggle, confirm) = match mode {
            crate::app::Marking::Complete => (
                "c",
                format!("confirm {} marked as completed", app.marked.len()),
            ),
            crate::app::Marking::Delete => (
                "d",
                format!("PERMANENTLY delete {} marked", app.marked.len()),
            ),
        };
        let mut hints = owned(&[(toggle, "mark"), ("↑/↓", "move")]);
        hints.push(("enter".to_string(), confirm));
        hints.extend(owned(&[("esc/q", "cancel")]));
        hints
    } else if let Some(Pending::NewSession { tools, .. }) = &app.pending {
        let pick = if tools.len() == 1 {
            "1".to_string()
        } else {
            format!("1-{}", tools.len().min(9))
        };
        let mut hints = owned(&[("↑/↓", "choose"), ("enter", "start")]);
        hints.push((pick, "direct pick".to_string()));
        hints.extend(owned(&[("esc/q", "cancel")]));
        hints
    } else if app.screen == Screen::Options {
        owned(&[
            ("↑/↓", "move"),
            ("enter", "set context window"),
            ("esc/o", "back"),
        ])
    } else {
        owned(&browse_hints(app))
    };
    hints.extend(owned(&[("ctrl+c", "quit")]));
    hints
}

const STATUS_SEPARATOR: &str = "   │   ";

fn fit_status(status: &str, hints: &[Span<'static>], width: usize, cap: u16) -> Option<String> {
    let tail: String = hints.iter().map(|span| span.content.as_ref()).collect();
    let fits = |text: &str| wrapped_rows(&format!(" {text}{STATUS_SEPARATOR}{tail}"), width) <= cap;
    if fits(status) {
        return Some(status.to_string());
    }
    let (mut low, mut high) = (0usize, status.chars().count());
    while low < high {
        let mid = low.midpoint(high + 1);
        if fits(&crate::core::truncate(status, mid)) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    (low > 1).then(|| crate::core::truncate(status, low))
}

fn browse_hints(app: &App) -> Vec<(&'static str, &'static str)> {
    let archived = app.screen == Screen::Archive || app.tree_of_archive();
    let mut hints = vec![("enter", "resume"), ("E", "run here")];
    hints.push(if archived {
        ("u", "reopen")
    } else {
        ("c", "mark as completed")
    });
    hints.extend([
        ("d", "mark for deletion"),
        ("m", "rename"),
        ("s", "set project"),
        ("n", "new session"),
    ]);
    if app.screen != Screen::Archive {
        hints.push(("a", "archive"));
    }
    hints.push(("v", app.tree_label()));
    if let Some(scope) = app.scope_label() {
        hints.push(("p", scope));
    }
    match app.screen {
        Screen::Archive => hints.push(("esc/a", "back")),
        Screen::Tree => hints.push(("esc", "back")),
        _ if app.has_filters() => hints.push(("esc", "clear filters")),
        _ => {}
    }
    hints.extend([
        ("f", "find"),
        ("i", "details"),
        ("l", "limits"),
        ("tab/shift+tab", "filter by tool"),
        ("↑/↓", "move"),
        ("home/end/G", "first/last"),
        ("o", "options"),
        ("r", "refresh"),
    ]);
    hints
}

fn hint_spans<'a>(hints: &'a [(&'a str, &'a str)]) -> Vec<Span<'static>> {
    let key_style = Style::new().fg(Color::Cyan).bold();
    let label_style = Style::new().fg(Color::Gray);
    let mnemonic_style = Style::new()
        .fg(Color::Yellow)
        .bold()
        .add_modifier(Modifier::UNDERLINED);
    let mut spans = Vec::new();
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::new().fg(Color::DarkGray)));
        }
        let mnemonic = key
            .chars()
            .next()
            .filter(|c| key.chars().count() == 1 && c.is_ascii_lowercase())
            .and_then(|k| {
                label
                    .char_indices()
                    .find(|(_, c)| c.eq_ignore_ascii_case(&k))
            });
        if let Some((pos, c)) = mnemonic {
            let (before, rest) = label.split_at(pos);
            let after = &rest[c.len_utf8()..];
            if !before.is_empty() {
                spans.push(Span::styled(before.to_string(), label_style));
            }
            spans.push(Span::styled(c.to_string(), mnemonic_style));
            if !after.is_empty() {
                spans.push(Span::styled(after.to_string(), label_style));
            }
        } else {
            spans.push(Span::styled(key.to_string(), key_style));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(label.to_string(), label_style));
        }
    }
    spans
}

fn repo_label(row: &SessionRow, max: usize) -> String {
    if let Some(alias) = row.project_label() {
        return truncate_left(&alias, max);
    }
    let Some(cwd) = row.session.cwd.as_deref() else {
        return "?".to_string();
    };
    let text = match dirs::home_dir().and_then(|home| {
        cwd.strip_prefix(&home)
            .ok()
            .map(std::path::Path::to_path_buf)
    }) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => cwd.display().to_string(),
    };
    truncate_left(&text, max)
}

fn truncate_left(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    let tail: String = s.chars().skip(count - max.saturating_sub(1)).collect();
    format!("…{tail}")
}

fn context_cell(row: &SessionRow) -> Cell<'static> {
    match (row.context_tokens.filter(|t| *t > 0), row.context_window) {
        (Some(tokens), Some(window)) if window > 0 => {
            let pct = (tokens as f64 / window as f64 * 100.0).min(100.0);
            Cell::from(Line::from(vec![
                Span::raw(format!(
                    "{:>13} ",
                    format!("{}/{}", format_tokens(tokens), format_tokens(window))
                )),
                Span::styled(usage_bar(pct, 6), Style::new().fg(usage_color(pct))),
            ]))
        }
        (Some(tokens), _) => Cell::from(format!("{:>13}", format_tokens(tokens))),
        _ => Cell::from(format!("{:>13}", "-")),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::app::Action;
    use crate::core::now_ms;
    use crate::limits::{ToolLimits, WindowUsage};

    fn app_with_limits(name: &str, resets_in_secs: i64) -> App {
        let dir = std::path::PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join(format!("screen-{name}.db"));
        let _ = std::fs::remove_file(&db);
        let store = crate::store::Store::open(Some(&db)).unwrap();
        let mut app = App::new(store, None, false);
        app.received_first_snapshot = true;
        app.limits = vec![ToolLimits {
            tool: crate::core::ToolId::Claude,
            windows: vec![WindowUsage {
                label: "5h".to_string(),
                used_percent: Some(24.0),
                resets_at: Some(now_ms() / 1000 + resets_in_secs),
                tokens: None,
                estimated: false,
                horizon_secs: Some(5 * 3600),
            }],
            plan: None,
            captured_at: now_ms(),
            source: "test".to_string(),
            note: None,
            stale_credentials: false,
        }];
        app
    }

    fn rendered_rows(app: &mut App, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).unwrap().symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn collapse(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn footer_text(app: &mut App, width: u16, height: u16) -> String {
        let rows = rendered_rows(app, width, height);
        let separator = rows
            .iter()
            .rposition(|line| !line.trim().is_empty() && line.trim().chars().all(|c| c == '─'))
            .unwrap_or_else(|| panic!("no separator row at {width}x{height}: {rows:#?}"));
        collapse(&rows[separator + 1..].join(" "))
    }

    fn hint_text(hint: (&str, &str)) -> String {
        collapse(
            &hint_spans(&[hint])
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
        )
    }

    fn probe_rows() -> Vec<crate::core::snapshot::SessionRow> {
        ["one", "two", "three"]
            .iter()
            .enumerate()
            .map(|(index, name)| crate::core::snapshot::SessionRow {
                project_alias: None,
                project: None,
                demo_archived: false,
                session: crate::core::SessionSummary {
                    key: crate::core::SessionKey {
                        tool: crate::core::ToolId::Claude,
                        id: format!("s{index}"),
                    },
                    cwd: Some(PathBuf::from(".")),
                    title: Some(format!("session {name}")),
                    preview: None,
                    model: None,
                    updated_at_ms: now_ms(),
                    native_tokens_used: None,
                    native_archived: false,
                    usage_files: Vec::new(),
                    created_at_ms: None,
                    origin_url: None,
                },
                liveness: Liveness::Idle,
                completed: index == 2,
                completed_at: (index == 2).then_some(1),
                usage: None,
                context_tokens: None,
                context_window: None,
            })
            .collect()
    }

    fn browse_app(name: &str, screen: Screen, prev: Screen, repo: bool) -> App {
        let ctx = repo.then(|| crate::core::repo::plain_directory_context(&PathBuf::from(".")));
        let dir = PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join(format!("hints-{name}.db"));
        let _ = std::fs::remove_file(&db);
        let store = crate::store::Store::open(Some(&db)).unwrap();
        let mut app = App::new(store, ctx, false);
        app.received_first_snapshot = true;
        app.rows = probe_rows();
        app.prev_screen = prev;
        app.screen = screen;
        app
    }

    const FOOTER_SIZES: [(u16, u16); 8] = [
        (60, 20),
        (72, 20),
        (80, 24),
        (90, 24),
        (100, 30),
        (120, 30),
        (160, 40),
        (200, 50),
    ];

    fn rendered_row_count(text: &str, width: u16) -> u16 {
        let height = 12u16;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new(Line::raw(text.to_string())).wrap(Wrap { trim: true }),
                    frame.area(),
                );
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .filter(|y| (0..width).any(|x| buffer.cell((x, *y)).unwrap().symbol().trim() != ""))
            .count() as u16
    }

    #[test]
    fn the_reserved_height_matches_what_ratatui_actually_wraps() {
        let mut app = browse_app("wrap-count", Screen::Monitor, Screen::Monitor, true);
        app.status = Some(
            "this session is archived inside Codex — press u to unarchive it first".to_string(),
        );
        let texts = [
            line_text(&footer_line(&app, 80, 4)),
            line_text(&footer_line(&app, 200, 4)),
            line_text(&header_line(&app, true)),
            "one two three four five six seven eight nine ten".to_string(),
            "a-single-word-longer-than-the-whole-terminal-width-by-far".to_string(),
            "  leading and trailing spaces  ".to_string(),
        ];
        for text in &texts {
            for width in [40u16, 57, 60, 72, 80, 100, 120, 160, 200] {
                let want = rendered_row_count(text, width);
                let got = wrapped_rows(text, width as usize);
                assert_eq!(
                    got, want,
                    "wrapped_rows disagrees with the renderer at width {width} for {text:?}"
                );
                assert_eq!(
                    wrapped_height(&Line::raw(text.clone()), width as usize, u16::MAX),
                    want,
                    "the reserved footer height is wrong at width {width} for {text:?}"
                );
            }
        }
    }

    #[test]
    fn a_status_message_does_not_hide_key_hints() {
        let mut missing: Vec<String> = Vec::new();
        for (width, height) in FOOTER_SIZES {
            let mut app = browse_app(
                &format!("status-{width}"),
                Screen::Monitor,
                Screen::Monitor,
                true,
            );
            app.status = Some(
                "this session is archived inside Codex — press u to unarchive it first".to_string(),
            );
            let hints = footer_hints(&app);
            let footer = footer_text(&mut app, width, height);
            for (key, label) in hints {
                let want = hint_text((key.as_str(), label.as_str()));
                if !footer.contains(&want) {
                    missing.push(format!("{width}x{height}: {want:?}"));
                }
            }
            assert!(
                footer.contains("this session is archived inside Codex"),
                "the status itself vanished at {width}x{height}: {footer:?}"
            );
        }
        assert!(
            missing.is_empty(),
            "a status message hid these hints:\n{}",
            missing.join("\n")
        );
    }

    type Setup = fn(&mut App);

    fn footer_states() -> Vec<(&'static str, Setup)> {
        vec![
            ("monitor", |_app| {}),
            ("filtered", |app| {
                app.filter = Some("session".to_string());
            }),
            ("repo", |app| {
                app.screen = Screen::Repo;
                app.prev_screen = Screen::Repo;
            }),
            ("archive", |app| {
                app.screen = Screen::Archive;
                app.prev_screen = Screen::Monitor;
            }),
            ("tree", |app| {
                app.screen = Screen::Tree;
                app.prev_screen = Screen::Monitor;
            }),
            ("archive-tree", |app| {
                app.screen = Screen::Tree;
                app.prev_screen = Screen::Archive;
            }),
            ("options", |app| {
                app.screen = Screen::Options;
                app.prev_screen = Screen::Monitor;
                app.refresh_options();
            }),
            ("limits-popup", |app| {
                app.show_limits = true;
            }),
            ("detail-popup", |app| {
                app.show_detail = true;
            }),
            ("marking-complete", |app| {
                app.marking = Some(crate::app::Marking::Complete);
            }),
            ("marking-delete", |app| {
                app.marking = Some(crate::app::Marking::Delete);
            }),
            ("new-session", |app| {
                app.pending = Some(Pending::NewSession {
                    tools: vec![
                        crate::core::ToolId::Claude,
                        crate::core::ToolId::Codex,
                        crate::core::ToolId::Kimi,
                    ],
                    selected: 0,
                });
            }),
            ("find", |app| {
                app.filter_input = Some(crate::app::LineEditor::new("ses".to_string()));
            }),
            ("rename", |app| {
                app.rename_input = Some(crate::app::RenameInput {
                    key: crate::core::SessionKey {
                        tool: crate::core::ToolId::Claude,
                        id: "s0".to_string(),
                    },
                    editor: crate::app::LineEditor::new("a new name".to_string()),
                });
            }),
            ("set-project", |app| {
                app.project_input = Some(crate::app::ProjectInput {
                    dir: PathBuf::from("/Users/someone/projects/a/deep/checkout"),
                    editor: crate::app::LineEditor::new("umbrella".to_string()),
                });
            }),
            ("context-window", |app| {
                app.window_input = Some(crate::app::WindowInput {
                    model: "opus".to_string(),
                    editor: crate::app::LineEditor::new("200K".to_string()),
                });
            }),
        ]
    }

    fn reset(app: &mut App) {
        app.screen = Screen::Monitor;
        app.prev_screen = Screen::Monitor;
        app.rows = probe_rows();
        app.filter = None;
        app.filter_input = None;
        app.window_input = None;
        app.rename_input = None;
        app.project_input = None;
        app.tool_filter = None;
        app.pending = None;
        app.status = None;
        app.marked.clear();
        app.marking = None;
        app.show_limits = false;
        app.show_detail = false;
        app.selected = 0;
        app.scroll = 0;
        app.options.clear();
        app.options_selected = 0;
        app.scan_status = None;
        app.scanning = false;
        app.last_ctrl_c = None;
    }

    fn fingerprint(app: &App) -> String {
        format!(
            "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}|{}|{}|{}|{}|{}|{}|{}|{:?}|{}|{}|{}|{:?}|{:?}|{:?}|{}|{}|{}|{}",
            app.screen,
            app.prev_screen,
            app.filter,
            app.tool_filter,
            app.scope_project,
            app.selected,
            app.scroll,
            app.marking,
            app.marked.len(),
            app.show_limits,
            app.show_detail,
            app.filter_input.is_some(),
            app.rename_input.is_some(),
            app.project_input.is_some(),
            app.window_input.is_some(),
            app.status,
            app.pending.is_some(),
            app.options.len(),
            app.options_selected,
            app.selected_key,
            app.scan_status,
            app.rows
                .iter()
                .map(|row| (row.completed, row.completed_at))
                .collect::<Vec<_>>(),
            app.scanning,
            app.last_ctrl_c.is_some(),
            app.pending_snapshot.is_some(),
            app.apply_next_snapshot,
        )
    }

    fn probe_keys() -> Vec<(KeyEvent, String)> {
        let mut keys = Vec::new();
        for c in 'a'..='z' {
            keys.push((
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                c.to_string(),
            ));
            let upper = c.to_ascii_uppercase();
            keys.push((
                KeyEvent::new(KeyCode::Char(upper), KeyModifiers::SHIFT),
                upper.to_string(),
            ));
            keys.push((
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
                format!("ctrl+{c}"),
            ));
        }
        for c in '1'..='9' {
            keys.push((
                KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
                c.to_string(),
            ));
        }
        for (code, name) in [
            (KeyCode::Tab, "tab"),
            (KeyCode::Esc, "esc"),
            (KeyCode::Enter, "enter"),
            (KeyCode::Up, "↑"),
            (KeyCode::Down, "↓"),
            (KeyCode::Left, "←"),
            (KeyCode::Right, "→"),
            (KeyCode::Home, "home"),
            (KeyCode::End, "end"),
            (KeyCode::Backspace, "backspace"),
            (KeyCode::Delete, "delete"),
        ] {
            keys.push((KeyEvent::new(code, KeyModifiers::NONE), name.to_string()));
        }
        keys.push((
            KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
            "shift+tab".to_string(),
        ));
        keys
    }

    fn hint_covers(hints: &[(String, String)], pressed: &str) -> bool {
        hints.iter().any(|(key, _)| {
            key == "any key"
                || key.split('/').any(|part| {
                    if part == pressed {
                        return true;
                    }
                    match (part.split_once('-'), pressed.chars().next()) {
                        (Some((first, last)), Some(digit)) if pressed.chars().count() == 1 => {
                            first.len() == 1
                                && last.len() == 1
                                && (first.chars().next().unwrap()..=last.chars().next().unwrap())
                                    .contains(&digit)
                        }
                        _ => false,
                    }
                })
        })
    }

    const TYPING_STATES: [&str; 4] = ["find", "rename", "set-project", "context-window"];

    const APP_SOURCE: &str = include_str!("../mod.rs");
    const MONITOR_SOURCE: &str = include_str!("monitor.rs");

    fn without_tests(source: &str) -> &str {
        source
            .find("#[cfg(test)]")
            .map_or(source, |cut| &source[..cut])
    }

    fn keys_bound_in(source: &str) -> std::collections::BTreeSet<String> {
        let mut keys = std::collections::BTreeSet::new();
        for (at, _) in source.match_indices("KeyCode::") {
            let rest = &source[at + "KeyCode::".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if name.is_empty() {
                continue;
            }
            if name != "Char" {
                keys.insert(name);
                continue;
            }
            let mut literal = rest[name.len()..].chars();
            if literal.next() != Some('(') || literal.next() != Some('\'') {
                continue;
            }
            let (Some(c), Some('\'')) = (literal.next(), literal.next()) else {
                continue;
            };
            keys.insert(format!("Char('{c}')"));
        }
        keys
    }

    fn block_after(source: &str, marker: &str, end: &str) -> String {
        let start = source
            .find(marker)
            .unwrap_or_else(|| panic!("{marker:?} is gone — this test needs updating"))
            + marker.len();
        let rest = &source[start..];
        let stop = rest
            .find(end)
            .unwrap_or_else(|| panic!("no end for {marker:?}"));
        rest[..stop].to_string()
    }

    fn field_names(block: &str) -> std::collections::BTreeSet<String> {
        block
            .lines()
            .filter_map(|line| line.trim().strip_prefix("pub "))
            .filter_map(|line| line.split_once(':'))
            .map(|(name, _)| name.trim().to_string())
            .collect()
    }

    #[test]
    fn the_key_probe_presses_every_key_the_app_binds() {
        let bound = keys_bound_in(without_tests(APP_SOURCE));
        assert!(
            bound.len() > 20 && bound.contains("Char('u')") && bound.contains("Tab"),
            "the key scan stopped finding bindings — it reads {bound:?}"
        );
        let probed: std::collections::BTreeSet<String> = probe_keys()
            .iter()
            .map(|(event, _)| format!("{:?}", event.code))
            .collect();
        let unpressed: Vec<&String> = bound
            .iter()
            .filter(|key| key.as_str() != "Char" && !probed.contains(*key))
            .collect();
        assert!(
            unpressed.is_empty(),
            "keys the app binds but no test ever presses: {unpressed:?}"
        );
    }

    #[test]
    fn the_fingerprint_watches_every_field_a_key_can_change() {
        const NOT_KEY_DRIVEN: [&str; 9] = [
            "store",
            "rows",
            "limits",
            "limits_disabled",
            "allowlist_mode",
            "repo",
            "repo_forced",
            "received_first_snapshot",
            "pending_snapshot",
        ];
        let fields: std::collections::BTreeSet<String> =
            field_names(&block_after(APP_SOURCE, "pub struct App {", "\n}"));
        assert!(
            fields.len() > 20 && fields.contains("screen"),
            "the field scan stopped finding App fields — it reads {fields:?}"
        );
        let watched = block_after(
            MONITOR_SOURCE,
            "fn fingerprint(app: &App) -> String {",
            "\n    }",
        );
        let unwatched: Vec<&String> = fields
            .iter()
            .filter(|field| !NOT_KEY_DRIVEN.contains(&field.as_str()))
            .filter(|field| !watched.contains(&format!("app.{field}")))
            .collect();
        assert!(
            unwatched.is_empty(),
            "a key could change these without any test noticing: {unwatched:?}"
        );
        let stale: Vec<&str> = NOT_KEY_DRIVEN
            .iter()
            .filter(|field| !fields.iter().any(|known| known == *field))
            .copied()
            .collect();
        assert!(
            stale.is_empty(),
            "exemptions for fields that are gone: {stale:?}"
        );
    }

    #[test]
    fn the_footer_states_reach_every_screen() {
        let variants: std::collections::BTreeSet<String> =
            block_after(APP_SOURCE, "pub enum Screen {", "\n}")
                .lines()
                .map(|line| line.trim().trim_end_matches(',').to_string())
                .filter(|line| !line.is_empty())
                .collect();
        assert!(
            variants.len() > 2 && variants.contains("Archive"),
            "the Screen scan stopped finding variants — it reads {variants:?}"
        );
        let mut app = browse_app("state-coverage", Screen::Monitor, Screen::Monitor, true);
        let mut reached = std::collections::BTreeSet::new();
        for (_, setup) in footer_states() {
            reset(&mut app);
            setup(&mut app);
            reached.insert(format!("{:?}", app.screen));
        }
        let unreached: Vec<&String> = variants.difference(&reached).collect();
        assert!(
            unreached.is_empty(),
            "no footer state renders these screens: {unreached:?}"
        );
    }

    #[test]
    fn every_key_that_does_something_is_advertised_in_the_footer() {
        let mut app = browse_app("coverage", Screen::Monitor, Screen::Monitor, true);
        let mut missing: Vec<String> = Vec::new();
        for (name, setup) in footer_states() {
            if TYPING_STATES.contains(&name) {
                continue;
            }
            for (event, pressed) in probe_keys() {
                reset(&mut app);
                setup(&mut app);
                let hints = footer_hints(&app);
                let before = fingerprint(&app);
                let action = app.handle_key(event);
                let acted = fingerprint(&app) != before || !matches!(action, Action::None);
                if acted && !hint_covers(&hints, &pressed) {
                    missing.push(format!("{name}: {pressed}"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "keys that act without a hint:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn every_key_hint_is_visible_in_every_state() {
        let mut missing: Vec<String> = Vec::new();
        for (name, setup) in footer_states() {
            for (width, height) in FOOTER_SIZES {
                let mut app = browse_app(
                    &format!("visible-{name}-{width}"),
                    Screen::Monitor,
                    Screen::Monitor,
                    true,
                );
                setup(&mut app);
                let hints = footer_hints(&app);
                let footer = footer_text(&mut app, width, height);
                for (key, label) in hints {
                    let want = hint_text((key.as_str(), label.as_str()));
                    if !footer.contains(&want) {
                        missing.push(format!("{name} at {width}x{height}: {want:?}"));
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "hints not rendered:\n{}",
            missing.join("\n")
        );
    }

    #[test]
    fn limits_render_on_their_own_row_with_a_reset_countdown() {
        for (width, height) in [(120, 30), (100, 18)] {
            let mut app = app_with_limits(&format!("row-{width}"), 2 * 3600);
            let rows = rendered_rows(&mut app, width, height);
            let limits_row = rows
                .iter()
                .position(|line| line.contains("limits"))
                .unwrap_or_else(|| panic!("no limits row at {width}x{height}: {rows:#?}"));
            let status_row = rows
                .iter()
                .position(|line| line.contains("RUNNING"))
                .unwrap_or_else(|| panic!("no status row at {width}x{height}"));
            assert_ne!(
                limits_row, status_row,
                "limits share the status row at {width}x{height}"
            );
            assert!(
                !rows[status_row].contains("limits"),
                "status row still carries limits at {width}x{height}"
            );
            assert!(
                rows[limits_row].contains('↻'),
                "no reset countdown at {width}x{height}: {:?}",
                rows[limits_row]
            );
        }
    }

    #[test]
    fn the_version_shows_next_to_the_title_in_both_header_modes() {
        let want = format!("v{}", env!("CARGO_PKG_VERSION"));
        for (width, height) in [(120, 30), (100, 18)] {
            let mut app = app_with_limits(&format!("ver-{width}"), 3600);
            let rows = rendered_rows(&mut app, width, height);
            let title_row = rows
                .iter()
                .find(|line| line.contains("szpont machen"))
                .unwrap_or_else(|| panic!("no title row at {width}x{height}: {rows:#?}"));
            assert!(
                title_row.contains(&want),
                "no {want} beside the title at {width}x{height}: {title_row:?}"
            );
        }
    }

    fn app_with_session(name: &str, title: &str, cwd: &str) -> App {
        let dir = std::path::PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join(format!("screen-{name}.db"));
        let _ = std::fs::remove_file(&db);
        let store = crate::store::Store::open(Some(&db)).unwrap();
        let mut app = App::new(store, None, false);
        app.received_first_snapshot = true;
        app.rows = vec![crate::core::snapshot::SessionRow {
            project_alias: None,
            project: None,
            demo_archived: false,
            session: crate::core::SessionSummary {
                key: crate::core::SessionKey {
                    tool: crate::core::ToolId::Claude,
                    id: "s1".to_string(),
                },
                cwd: Some(std::path::PathBuf::from(cwd)),
                title: Some(title.to_string()),
                preview: None,
                model: None,
                updated_at_ms: now_ms(),
                native_tokens_used: None,
                native_archived: false,
                usage_files: Vec::new(),
                created_at_ms: None,
                origin_url: None,
            },
            liveness: Liveness::Idle,
            completed: false,
            completed_at: None,
            usage: None,
            context_tokens: None,
            context_window: None,
        }];
        app
    }

    #[test]
    fn title_and_repo_share_the_leftover_width_evenly_when_both_want_it() {
        let title = "a session title so long that it wants more room than half of the leftover space could ever give it";
        let cwd = "/Users/someone/projects/deeply/nested/checkout/of/a/repository";
        for width in [120u16, 150, 200] {
            let mut app = app_with_session(&format!("width-{width}"), title, cwd);
            let (title_span, repo_span) = column_spans(&mut app, width);
            assert!(
                title_span.abs_diff(repo_span) <= 1,
                "columns not evenly split at {width}: title {title_span}, repo {repo_span}"
            );
        }
    }

    fn column_spans(app: &mut App, width: u16) -> (usize, usize) {
        let rows = rendered_rows(app, width, 12);
        let header = rows
            .iter()
            .find(|line| line.contains("TITLE") && line.contains("REPO"))
            .unwrap_or_else(|| panic!("no header at {width}"));
        let title_col = header.find("TITLE").unwrap();
        let repo_col = header.find("REPO").unwrap();
        let tokens_col = header.find("TOKENS").unwrap();
        (repo_col - title_col, tokens_col - repo_col)
    }

    #[test]
    fn the_title_column_shrinks_to_the_longest_title_and_repo_takes_the_slack() {
        let cwd = "/Users/someone/projects/deeply/nested/checkout/of/a/repository";
        let mut short = app_with_session("fit-short", "tiny title", cwd);
        let (short_title, short_repo) = column_spans(&mut short, 150);

        let long = "a distinctive session title that is really quite long indeed";
        let mut wide = app_with_session("fit-long", long, cwd);
        let (long_title, long_repo) = column_spans(&mut wide, 150);

        assert!(
            short_title < long_title,
            "short title did not shrink its column: {short_title} vs {long_title}"
        );
        assert!(
            short_repo > long_repo,
            "repo did not take the slack: {short_repo} vs {long_repo}"
        );
        assert!(
            short_title >= "tiny title".len(),
            "title column narrower than the title itself: {short_title}"
        );
    }

    #[test]
    fn a_short_title_does_not_reserve_width_it_cannot_use() {
        let cwd = "/Users/someone/projects/deeply/nested/checkout/of/a/repository";
        let mut app = app_with_session("short-title", "hi", cwd);
        let rows = rendered_rows(&mut app, 150, 12).join("\n");
        assert!(
            rows.contains("repository"),
            "repo tail should fit beside a short title:\n{rows}"
        );
    }

    #[test]
    fn project_colors_are_stable_and_spread_across_the_palette() {
        let names = [
            "dboard",
            "reanimated",
            "szpont",
            "vscode",
            "worklets",
            "analysis",
            "hud",
            "canbus",
        ];
        for name in names {
            assert_eq!(
                super::project_color(name),
                super::project_color(name),
                "{name} changed colour between calls"
            );
        }
        let distinct: std::collections::HashSet<_> =
            names.iter().map(|n| super::project_color(n)).collect();
        assert!(
            distinct.len() >= names.len() - 2,
            "palette collapsed: {} colours for {} names",
            distinct.len(),
            names.len()
        );
        assert_ne!(super::project_color("alpha"), super::project_color("beta"));
    }

    #[test]
    fn a_project_name_keeps_its_colour_across_versions() {
        assert_eq!(super::project_color("dboard"), Color::Indexed(170));
        assert_eq!(super::project_color("szpont"), Color::Indexed(76));
        assert_eq!(super::project_color("reanimated"), Color::Indexed(178));
    }
}
