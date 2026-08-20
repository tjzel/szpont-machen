use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, List, ListItem, Paragraph, Wrap};

use crate::app::App;
use crate::core::format_tokens;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let width = frame.area().width.max(1) as usize;
    let cap = super::monitor::footer_cap(frame.area().height);
    let footer = super::monitor::footer_line(app, width, cap);
    let footer_height = super::monitor::wrapped_height(&footer, width, cap);
    let [header_area, body_area, separator_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(footer_height),
    ])
    .areas(frame.area());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            crate::app::widgets::logo::compact(),
            Span::raw("  "),
            Span::styled("options", Style::new().fg(Color::Cyan).bold()),
        ])),
        header_area,
    );
    let block = Block::bordered()
        .border_style(Style::new().fg(Color::Cyan))
        .title(" options ")
        .title_style(Style::new().fg(Color::Cyan).bold());
    let inner = block.inner(body_area);
    frame.render_widget(block, body_area);
    if app.options.is_empty() {
        frame.render_widget(
            Paragraph::new("no models discovered yet — options appear after the first scan")
                .style(Style::new().fg(Color::DarkGray))
                .centered(),
            inner,
        );
    } else {
        let items: Vec<ListItem> = app
            .options
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let window = match option.window {
                    Some(w) => format_tokens(w),
                    None => "not set".to_string(),
                };
                let source = if option.overridden {
                    "set by you"
                } else if option.window.is_some() {
                    "reported by the tool"
                } else {
                    ""
                };
                let mut style = Style::new();
                if i == app.options_selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                ListItem::new(Line::from(vec![
                    Span::raw(format!(" {:<28}", crate::core::truncate(&option.model, 28))),
                    Span::styled("context window: ", Style::new().fg(Color::DarkGray)),
                    Span::styled(format!("{window:<10}"), Style::new().fg(Color::Cyan)),
                    Span::styled(source, Style::new().fg(Color::DarkGray)),
                ]))
                .style(style)
            })
            .collect();
        frame.render_widget(List::new(items), inner);
    }
    frame.render_widget(
        Paragraph::new("─".repeat(frame.area().width as usize))
            .style(Style::new().fg(Color::DarkGray)),
        separator_area,
    );
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: true }),
        footer_area,
    );
}
