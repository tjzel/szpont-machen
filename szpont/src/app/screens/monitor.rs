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
    let footer = footer_line(app);
    let footer_height = wrapped_height(footer.width(), width, MAX_FOOTER_LINES);
    let status_height = wrapped_height(header_line(app, true).width(), width, MAX_HEADER_LINES);
    let limits_height = wrapped_height(limits_line(app).width(), width, MAX_HEADER_LINES);
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

const MAX_FOOTER_LINES: u16 = 3;
const MAX_HEADER_LINES: u16 = 2;

fn wrapped_height(content_width: usize, area_width: usize, cap: u16) -> u16 {
    let lines = content_width.div_ceil(area_width.max(1)).max(1);
    (lines as u16).min(cap)
}

fn footer_line(app: &App) -> Line<'static> {
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
        spans.extend(hint_spans(&[("enter", "keep"), ("esc", "cancel")]));
    } else if let Some(input) = &app.window_input {
        spans.push(Span::styled(
            format!("context window for {}: ", input.model),
            status_style,
        ));
        spans.extend(input.editor.cursor_spans(status_style));
        spans.push(Span::raw("   "));
        spans.extend(hint_spans(&[("enter", "save"), ("esc", "cancel")]));
    } else if let Some(input) = &app.rename_input {
        spans.push(Span::styled("rename: ", status_style));
        spans.extend(input.editor.cursor_spans(status_style));
        spans.push(Span::raw("   "));
        spans.extend(hint_spans(&[("enter", "save"), ("esc", "cancel")]));
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
        spans.extend(hint_spans(&[("enter", "save"), ("esc", "cancel")]));
    } else if let Some(mode) = app.marking {
        match mode {
            crate::app::Marking::Complete => {
                let confirm_label = format!("confirm {} marked as completed", app.marked.len());
                spans.extend(hint_spans(&[
                    ("c", "mark"),
                    ("↑/↓", "move"),
                    ("enter", confirm_label.as_str()),
                    ("esc", "cancel"),
                ]));
            }
            crate::app::Marking::Delete => {
                let confirm_label = format!("PERMANENTLY delete {} marked", app.marked.len());
                spans.extend(hint_spans(&[
                    ("d", "mark"),
                    ("↑/↓", "move"),
                    ("enter", confirm_label.as_str()),
                    ("esc", "cancel"),
                ]));
            }
        }
    } else if let Some(Pending::NewSession { tools, .. }) = &app.pending {
        let pick_label = if tools.len() == 1 {
            "1".to_string()
        } else {
            format!("1-{}", tools.len().min(9))
        };
        spans.extend(hint_spans(&[
            ("↑/↓", "choose"),
            ("enter", "start"),
            (pick_label.as_str(), "direct pick"),
            ("esc", "cancel"),
        ]));
    } else {
        if let Some(status) = &app.status {
            spans.push(Span::styled(status.clone(), status_style));
            spans.push(Span::styled("   │   ", Style::new().fg(Color::DarkGray)));
        }
        let tree = app.tree_label();
        let hints: Vec<(&str, &str)> = if app.screen == Screen::Archive {
            vec![
                ("enter", "resume"),
                ("u", "reopen"),
                ("d", "mark for deletion"),
                ("m", "rename"),
                ("s", "set project"),
                ("v", tree),
                ("esc", "back"),
                ("ctrl+c", "quit"),
            ]
        } else if app.screen == Screen::Tree {
            let complete_hint: (&str, &str) = if app.tree_of_archive() {
                ("u", "reopen")
            } else {
                ("c", "mark as completed")
            };
            vec![
                ("enter", "resume"),
                complete_hint,
                ("d", "mark for deletion"),
                ("m", "rename"),
                ("v", tree),
                ("f", "find"),
                ("esc", "back"),
                ("ctrl+c", "quit"),
            ]
        } else {
            let mut h = vec![
                ("enter", "resume"),
                ("c", "mark as completed"),
                ("d", "mark for deletion"),
                ("m", "rename"),
                ("n", "new session"),
                ("a", "archive"),
                ("v", tree),
            ];
            if let Some(scope) = app.scope_label() {
                h.push(("p", scope));
            }
            h.extend([
                ("f", "find"),
                ("o", "options"),
                ("r", "refresh"),
                ("ctrl+c", "quit"),
            ]);
            h
        };
        spans.extend(hint_spans(&hints));
    }
    Line::from(spans)
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
            .filter(|_| key.chars().count() == 1)
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

    use super::*;
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
