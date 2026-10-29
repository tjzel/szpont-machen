pub mod run;
mod screens;
mod widgets;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;

use crate::adapters;
use crate::core::repo::RepoContext;
use crate::core::snapshot::SessionRow;
use crate::core::{LaunchSpec, SessionKey, ToolId};
use crate::store::Store;

pub enum AppEvent {
    Snapshot(Vec<SessionRow>),
    Limits(Vec<crate::limits::ToolLimits>),
    Progress(String),
    ScanDone(String),
    ScanError(String),
}

pub enum Action {
    None,
    Quit,
    Launch { spec: LaunchSpec, replace: bool },
    Refresh,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Screen {
    Monitor,
    Repo,
    Archive,
    Tree,
    Options,
}

pub struct OptionRow {
    pub model: String,
    pub window: Option<u64>,
    pub overridden: bool,
}

pub enum Pending {
    NewSession { tools: Vec<ToolId>, selected: usize },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Marking {
    Complete,
    Delete,
}

const MAX_EDITOR_CHARS: usize = 512;
const MAX_PROJECT_CHARS: usize = 32;

pub struct LineEditor {
    pub buffer: String,
    pub cursor: usize,
}

impl LineEditor {
    pub fn new(buffer: String) -> LineEditor {
        let buffer = if buffer.chars().count() > MAX_EDITOR_CHARS {
            buffer.chars().take(MAX_EDITOR_CHARS).collect()
        } else {
            buffer
        };
        let cursor = buffer.chars().count();
        LineEditor { buffer, cursor }
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_index)
            .map_or(self.buffer.len(), |(pos, _)| pos)
    }

    pub fn handle(&mut self, key: KeyEvent) -> bool {
        let mods = key.modifiers;
        let shortcut = mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::SUPER);
        match key.code {
            KeyCode::Char(c) if shortcut => match c {
                'u' => self.delete_to_start(),
                'w' => self.delete_word_back(),
                'h' => self.backspace(),
                'a' => {
                    self.cursor = 0;
                    false
                }
                'e' => {
                    self.cursor = self.buffer.chars().count();
                    false
                }
                _ => false,
            },
            KeyCode::Char(_) if mods.contains(KeyModifiers::ALT) => false,
            KeyCode::Char(c) => {
                if self.buffer.chars().count() >= MAX_EDITOR_CHARS {
                    return false;
                }
                let at = self.byte_at(self.cursor);
                self.buffer.insert(at, c);
                self.cursor += 1;
                true
            }
            KeyCode::Backspace => {
                if mods.contains(KeyModifiers::SUPER) {
                    self.delete_to_start()
                } else if mods.contains(KeyModifiers::CONTROL) || mods.contains(KeyModifiers::ALT) {
                    self.delete_word_back()
                } else {
                    self.backspace()
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.buffer.chars().count() {
                    let at = self.byte_at(self.cursor);
                    self.buffer.remove(at);
                    true
                } else {
                    false
                }
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                false
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.buffer.chars().count());
                false
            }
            KeyCode::Home => {
                self.cursor = 0;
                false
            }
            KeyCode::End => {
                self.cursor = self.buffer.chars().count();
                false
            }
            _ => false,
        }
    }

    fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.cursor -= 1;
        let at = self.byte_at(self.cursor);
        self.buffer.remove(at);
        true
    }

    fn delete_to_start(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let at = self.byte_at(self.cursor);
        self.buffer.replace_range(..at, "");
        self.cursor = 0;
        true
    }

    fn delete_word_back(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        let chars: Vec<char> = self.buffer.chars().collect();
        let mut target = self.cursor;
        while target > 0 && chars[target - 1].is_whitespace() {
            target -= 1;
        }
        while target > 0 && !chars[target - 1].is_whitespace() {
            target -= 1;
        }
        let start = self.byte_at(target);
        let end = self.byte_at(self.cursor);
        self.buffer.replace_range(start..end, "");
        self.cursor = target;
        true
    }

    pub fn cursor_spans(&self, base: Style) -> Vec<Span<'static>> {
        let at = self.byte_at(self.cursor);
        let mut rest = self.buffer[at..].chars();
        let under = rest.next().map_or_else(|| " ".to_string(), String::from);
        vec![
            Span::styled(self.buffer[..at].to_string(), base),
            Span::styled(under, base.add_modifier(Modifier::REVERSED)),
            Span::styled(rest.as_str().to_string(), base),
        ]
    }
}

pub struct WindowInput {
    pub model: String,
    pub editor: LineEditor,
}

pub struct RenameInput {
    pub key: SessionKey,
    pub editor: LineEditor,
}

pub struct ProjectInput {
    pub dir: PathBuf,
    pub editor: LineEditor,
}

pub struct App {
    pub rows: Vec<SessionRow>,
    pub limits: Vec<crate::limits::ToolLimits>,
    pub show_limits: bool,
    pub show_detail: bool,
    pub filter: Option<String>,
    pub filter_input: Option<LineEditor>,
    pub window_input: Option<WindowInput>,
    pub rename_input: Option<RenameInput>,
    pub project_input: Option<ProjectInput>,
    pub scope_project: Option<String>,
    pub tool_filter: Option<ToolId>,
    pub screen: Screen,
    pub prev_screen: Screen,
    pub repo: Option<RepoContext>,
    pub repo_forced: bool,
    pub selected: usize,
    pub selected_key: Option<SessionKey>,
    pub pending: Option<Pending>,
    pub status: Option<String>,
    pub store: Store,
    pub received_first_snapshot: bool,
    pub last_ctrl_c: Option<Instant>,
    pub marked: HashSet<SessionKey>,
    pub marking: Option<Marking>,
    pub scroll: usize,
    pub options: Vec<OptionRow>,
    pub options_selected: usize,
    pub pending_snapshot: Option<Vec<SessionRow>>,
    pub apply_next_snapshot: bool,
    pub scan_status: Option<String>,
    pub scanning: bool,
    pub allowlist_mode: bool,
    pub limits_disabled: bool,
}

impl App {
    pub fn new(store: Store, repo: Option<RepoContext>, repo_forced: bool) -> App {
        let screen = if repo.is_some() {
            Screen::Repo
        } else {
            Screen::Monitor
        };
        let scope_project = repo.as_ref().and_then(|ctx| {
            store
                .project_dirs()
                .ok()
                .and_then(|dirs| crate::core::snapshot::project_for(&dirs, &ctx.root))
        });
        App {
            rows: Vec::new(),
            limits: Vec::new(),
            show_limits: false,
            show_detail: false,
            filter: None,
            filter_input: None,
            window_input: None,
            rename_input: None,
            project_input: None,
            scope_project,
            tool_filter: None,
            screen,
            prev_screen: screen,
            repo,
            repo_forced,
            selected: 0,
            selected_key: None,
            pending: None,
            status: None,
            store,
            received_first_snapshot: false,
            last_ctrl_c: None,
            marked: HashSet::new(),
            marking: None,
            scroll: 0,
            options: Vec::new(),
            options_selected: 0,
            pending_snapshot: None,
            apply_next_snapshot: false,
            scan_status: None,
            scanning: false,
            allowlist_mode: false,
            limits_disabled: false,
        }
    }

    pub fn tree_of_archive(&self) -> bool {
        self.screen == Screen::Tree && self.prev_screen == Screen::Archive
    }

    pub fn tree_label(&self) -> &'static str {
        if self.screen == Screen::Tree {
            "flat view"
        } else {
            "tree view"
        }
    }

    pub fn has_filters(&self) -> bool {
        self.filter.is_some() || self.tool_filter.is_some() || !self.marked.is_empty()
    }

    pub fn scope_label(&self) -> Option<&'static str> {
        match self.screen {
            Screen::Repo => Some("global scope"),
            Screen::Monitor if self.repo.is_some() => Some("local scope"),
            _ => None,
        }
    }

    fn row_visible(&self, r: &SessionRow) -> bool {
        let screen_ok = match self.screen {
            Screen::Archive => r.completed,
            Screen::Tree => {
                if self.tree_of_archive() {
                    r.completed
                } else {
                    !r.completed
                }
            }
            _ => !r.completed,
        };
        let repo_ok = match (self.screen, &self.repo) {
            (Screen::Repo, Some(ctx)) => match &self.scope_project {
                Some(project) => r.project.as_deref() == Some(project.as_str()),
                None => ctx.matches(r),
            },
            _ => true,
        };
        let tool_ok = self
            .tool_filter
            .is_none_or(|tool| r.session.key.tool == tool);
        screen_ok && repo_ok && tool_ok
    }

    fn active_query(&self) -> Option<&str> {
        self.filter_input
            .as_ref()
            .map(|editor| editor.buffer.as_str())
            .or(self.filter.as_deref())
            .map(str::trim)
            .filter(|q| !q.is_empty())
    }

    fn filtered_indices(&self, rows: &[SessionRow]) -> Vec<usize> {
        let query = self.active_query();
        let mut matched: Vec<(usize, i64)> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| self.row_visible(r))
            .filter_map(|(i, r)| match query {
                None => Some((i, 0)),
                Some(q) => {
                    let haystack = format!(
                        "{} {} {} {}",
                        r.session.key.tool.as_str(),
                        r.session.title.as_deref().unwrap_or(""),
                        r.session.preview.as_deref().unwrap_or(""),
                        r.project_label().unwrap_or_else(|| {
                            r.session
                                .cwd
                                .as_deref()
                                .map(|p| p.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        })
                    );
                    crate::core::fuzzy_score(q, &haystack).map(|score| (i, score))
                }
            })
            .collect();
        if query.is_some() {
            matched.sort_by_key(|(_, score)| -score);
        }
        if self.screen == Screen::Tree {
            matched.sort_by_cached_key(|&(i, _)| {
                (
                    row_tree_components(&rows[i]),
                    std::cmp::Reverse(rows[i].session.updated_at_ms),
                )
            });
        }
        matched.into_iter().map(|(i, _)| i).collect()
    }

    pub fn visible_indices(&self) -> Vec<usize> {
        self.filtered_indices(&self.rows)
    }

    fn visible_keys_of<'a>(&self, rows: &'a [SessionRow]) -> Vec<&'a SessionKey> {
        self.filtered_indices(rows)
            .into_iter()
            .map(|i| &rows[i].session.key)
            .collect()
    }

    pub fn selected_row(&self) -> Option<&SessionRow> {
        let visible = self.visible_indices();
        visible.get(self.selected).map(|&i| &self.rows[i])
    }

    pub fn apply_snapshot(&mut self, rows: Vec<SessionRow>) {
        if self.received_first_snapshot && !self.apply_next_snapshot {
            let unchanged_order = self.visible_keys_of(&self.rows) == self.visible_keys_of(&rows);
            if !unchanged_order {
                self.pending_snapshot = Some(rows);
                return;
            }
        }
        self.apply_next_snapshot = false;
        self.pending_snapshot = None;
        self.install_snapshot(rows);
    }

    pub fn load_pending_snapshot(&mut self) {
        if let Some(rows) = self.pending_snapshot.take() {
            self.install_snapshot(rows);
        }
    }

    fn install_snapshot(&mut self, mut rows: Vec<SessionRow>) {
        let first = !self.received_first_snapshot;
        match self.store.custom_titles() {
            Ok(titles) if !titles.is_empty() => {
                for row in &mut rows {
                    if let Some(title) = titles.get(&row.session.key) {
                        row.session.title = Some(title.clone());
                    }
                }
            }
            Ok(_) => {}
            Err(err) => crate::logging::warn(&format!("cannot read custom titles: {err:#}")),
        }
        self.rows = rows;
        self.received_first_snapshot = true;
        if first
            && self.screen == Screen::Repo
            && !self.repo_forced
            && self.visible_indices().is_empty()
        {
            self.screen = Screen::Monitor;
            self.status = Some(
                "no sessions in this repo yet — showing all sessions (press n to start one here)"
                    .to_string(),
            );
        }
        let visible = self.visible_indices();
        if let Some(key) = &self.selected_key
            && let Some(pos) = visible
                .iter()
                .position(|&i| &self.rows[i].session.key == key)
        {
            self.selected = pos;
            return;
        }
        self.selected = self.selected.min(visible.len().saturating_sub(1));
        self.remember_selection();
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self
                .last_ctrl_c
                .is_some_and(|at| at.elapsed() < Duration::from_secs(2))
            {
                return Action::Quit;
            }
            self.last_ctrl_c = Some(Instant::now());
            self.status = Some("press ctrl+c again to exit".to_string());
            return Action::None;
        }
        if self.show_limits {
            self.show_limits = false;
            return Action::None;
        }
        if self.show_detail {
            self.show_detail = false;
            return Action::None;
        }
        if self.filter_input.is_some() {
            return self.handle_filter_key(key);
        }
        if self.window_input.is_some() {
            return self.handle_window_input_key(key);
        }
        if self.rename_input.is_some() {
            return self.handle_rename_key(key);
        }
        if self.project_input.is_some() {
            return self.handle_project_key(key);
        }
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER)
        {
            return Action::None;
        }
        if self.pending.is_some() {
            return self.handle_pending_key(key);
        }
        if let Some(mode) = self.marking {
            return self.handle_marking_key(mode, key);
        }
        if self.screen == Screen::Options {
            return self.handle_options_key(key);
        }
        match key.code {
            KeyCode::Char('l') => {
                self.show_limits = true;
                Action::None
            }
            KeyCode::Char('i') => {
                if self.selected_row().is_some() {
                    self.show_detail = true;
                }
                Action::None
            }
            KeyCode::Char('f') => {
                self.filter_input = Some(LineEditor::new(self.filter.clone().unwrap_or_default()));
                Action::None
            }
            KeyCode::Esc => {
                if self.screen == Screen::Archive {
                    self.toggle_archive();
                } else if self.screen == Screen::Tree {
                    self.screen = self.return_screen(Screen::Tree);
                    self.selected = 0;
                    self.remember_selection();
                } else if !self.marked.is_empty() {
                    self.marked.clear();
                } else {
                    let cleared_filter = self.filter.take().is_some();
                    let cleared_tool = self.tool_filter.take().is_some();
                    if cleared_filter || cleared_tool {
                        self.selected = 0;
                        self.remember_selection();
                    }
                }
                Action::None
            }
            KeyCode::Tab => {
                self.cycle_tool_filter(1);
                Action::None
            }
            KeyCode::BackTab => {
                self.cycle_tool_filter(-1);
                Action::None
            }
            KeyCode::Down => {
                self.move_selection(1);
                Action::None
            }
            KeyCode::Up => {
                self.move_selection(-1);
                Action::None
            }
            KeyCode::Home => {
                self.selected = 0;
                self.remember_selection();
                Action::None
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.selected = self.visible_indices().len().saturating_sub(1);
                self.remember_selection();
                Action::None
            }
            KeyCode::Enter => self.launch_selected(true),
            KeyCode::Char('E') => self.launch_selected(false),
            KeyCode::Char('a') => {
                self.toggle_archive();
                Action::None
            }
            KeyCode::Char('s') => {
                let Some(row) = self.selected_row() else {
                    return Action::None;
                };
                let Some(dir) = row.session.cwd.clone() else {
                    self.status = Some("this session has no directory to assign".to_string());
                    return Action::None;
                };
                let current = row.project.clone().unwrap_or_default();
                self.project_input = Some(ProjectInput {
                    dir,
                    editor: LineEditor::new(current),
                });
                Action::None
            }
            KeyCode::Char('u') => {
                if self.screen == Screen::Archive || self.tree_of_archive() {
                    self.unmark_selected();
                    Action::Refresh
                } else {
                    Action::None
                }
            }
            KeyCode::Char('n') => {
                let tools: Vec<ToolId> = adapters::all()
                    .iter()
                    .filter(|a| a.is_installed() || adapters::program_on_path(a.id().as_str()))
                    .map(|a| a.id())
                    .collect();
                if tools.is_empty() {
                    self.status = Some("no AI CLI tools found on this machine".to_string());
                } else {
                    self.pending = Some(Pending::NewSession { tools, selected: 0 });
                }
                Action::None
            }
            KeyCode::Char('c') => {
                if self.screen != Screen::Archive
                    && !self.tree_of_archive()
                    && self.selected_row().is_some()
                {
                    self.marking = Some(Marking::Complete);
                    self.marked.clear();
                    self.status = None;
                    self.toggle_mark_and_advance();
                }
                Action::None
            }
            KeyCode::Char('d') => {
                if self.selected_row().is_some() {
                    self.marking = Some(Marking::Delete);
                    self.marked.clear();
                    self.status = None;
                    self.toggle_mark_and_advance();
                }
                Action::None
            }
            KeyCode::Char('v') => {
                if self.screen == Screen::Tree {
                    self.screen = self.return_screen(Screen::Tree);
                } else {
                    self.prev_screen = self.screen;
                    self.screen = Screen::Tree;
                }
                self.selected = 0;
                self.remember_selection();
                Action::None
            }
            KeyCode::Char('p') => {
                match self.screen {
                    Screen::Monitor if self.repo.is_some() => {
                        self.screen = Screen::Repo;
                        self.selected = 0;
                        self.remember_selection();
                    }
                    Screen::Repo => {
                        self.screen = Screen::Monitor;
                        self.selected = 0;
                        self.remember_selection();
                    }
                    Screen::Monitor => {
                        self.status = Some(
                            "szpont could not read its working directory — local scope unavailable"
                                .to_string(),
                        );
                    }
                    _ => {}
                }
                Action::None
            }
            KeyCode::Char('r') => {
                self.load_pending_snapshot();
                self.scan_status = Some("rescanning…".to_string());
                self.scanning = true;
                Action::Refresh
            }
            KeyCode::Char('o') => {
                self.prev_screen = self.screen;
                self.screen = Screen::Options;
                self.options_selected = 0;
                self.refresh_options();
                Action::None
            }
            KeyCode::Char('m') => {
                if let Some(row) = self.selected_row() {
                    self.rename_input = Some(RenameInput {
                        key: row.session.key.clone(),
                        editor: LineEditor::new(row.session.title.clone().unwrap_or_default()),
                    });
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_marking_key(&mut self, mode: Marking, key: KeyEvent) -> Action {
        let toggle_key = match mode {
            Marking::Complete => 'c',
            Marking::Delete => 'd',
        };
        match key.code {
            KeyCode::Char(c) if c == toggle_key => {
                self.toggle_mark_and_advance();
                Action::None
            }
            KeyCode::Down => {
                self.move_selection(1);
                Action::None
            }
            KeyCode::Up => {
                self.move_selection(-1);
                Action::None
            }
            KeyCode::Enter => {
                let targets: Vec<SessionKey> = self.marked.iter().cloned().collect();
                self.marking = None;
                self.marked.clear();
                self.status = None;
                if targets.is_empty() {
                    return Action::None;
                }
                match mode {
                    Marking::Complete => self.complete_targets(&targets),
                    Marking::Delete => self.delete_targets(&targets),
                }
                let len = self.visible_indices().len();
                self.selected = self.selected.min(len.saturating_sub(1));
                self.remember_selection();
                Action::Refresh
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.marking = None;
                self.marked.clear();
                self.status = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    fn complete_targets(&mut self, targets: &[SessionKey]) {
        let mut done = 0;
        for session_key in targets {
            match self
                .store
                .mark_completed(session_key.tool, &session_key.id, "tui")
            {
                Ok(()) => {
                    done += 1;
                    if let Some(row) = self.rows.iter_mut().find(|r| r.session.key == *session_key)
                    {
                        row.completed = true;
                    }
                }
                Err(err) => self.status = Some(format!("error: {err:#}")),
            }
        }
        if self.status.is_none() {
            self.status = Some(if done == 1 {
                "session marked as completed".to_string()
            } else {
                format!("{done} sessions marked as completed")
            });
        }
    }

    fn delete_targets(&mut self, targets: &[SessionKey]) {
        let (open, deletable): (Vec<&SessionKey>, Vec<&SessionKey>) =
            targets.iter().partition(|k| {
                self.rows
                    .iter()
                    .find(|r| r.session.key == **k)
                    .is_some_and(|r| r.liveness != crate::core::Liveness::Idle)
            });
        let mut deleted: HashSet<SessionKey> = HashSet::new();
        let all_adapters = adapters::all();
        for session_key in deletable {
            let Some(row) = self.rows.iter().find(|r| r.session.key == *session_key) else {
                continue;
            };
            let Some(adapter) = all_adapters.iter().find(|a| a.id() == session_key.tool) else {
                continue;
            };
            let result = adapter.delete_session(&row.session).and_then(|()| {
                self.store
                    .delete_session_data(session_key.tool, &session_key.id)
            });
            match result {
                Ok(()) => {
                    deleted.insert(session_key.clone());
                }
                Err(err) => {
                    self.status = Some(format!("delete failed for {}: {err:#}", session_key.id));
                }
            }
        }
        self.rows.retain(|r| !deleted.contains(&r.session.key));
        if self.status.is_none() {
            let mut message = format!("{} session(s) deleted", deleted.len());
            if !open.is_empty() {
                message = format!(
                    "{message}, {} skipped (still open in their tool)",
                    open.len()
                );
            }
            self.status = Some(message);
        }
    }

    fn toggle_mark_and_advance(&mut self) {
        if let Some(row) = self.selected_row() {
            let session_key = row.session.key.clone();
            if !self.marked.remove(&session_key) {
                self.marked.insert(session_key);
            }
            self.move_selection(1);
        }
    }

    fn handle_filter_key(&mut self, key: KeyEvent) -> Action {
        let Some(mut input) = self.filter_input.take() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Enter => {
                self.filter = Some(input.buffer).filter(|s| !s.is_empty());
                self.selected = 0;
                self.remember_selection();
            }
            KeyCode::Esc => {
                self.selected = 0;
                self.remember_selection();
            }
            _ => {
                let changed = input.handle(key);
                self.filter_input = Some(input);
                if changed {
                    self.selected = 0;
                    self.remember_selection();
                }
            }
        }
        Action::None
    }

    pub fn refresh_options(&mut self) {
        let overrides = self.store.context_window_overrides().unwrap_or_default();
        let mut models: Vec<String> = self
            .rows
            .iter()
            .filter_map(|r| r.session.model.clone())
            .collect();
        models.sort();
        models.dedup();
        self.options = models
            .into_iter()
            .map(|model| {
                let overridden = overrides.contains_key(&model);
                let window = overrides.get(&model).copied().or_else(|| {
                    self.rows
                        .iter()
                        .filter(|r| r.session.model.as_deref() == Some(&model))
                        .find_map(|r| r.context_window)
                });
                OptionRow {
                    model,
                    window,
                    overridden,
                }
            })
            .collect();
        self.options_selected = self
            .options_selected
            .min(self.options.len().saturating_sub(1));
    }

    fn handle_options_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('o') => {
                self.screen = self.prev_screen;
                Action::None
            }
            KeyCode::Down => {
                self.options_selected =
                    (self.options_selected + 1).min(self.options.len().saturating_sub(1));
                Action::None
            }
            KeyCode::Up => {
                self.options_selected = self.options_selected.saturating_sub(1);
                Action::None
            }
            KeyCode::Enter => {
                if let Some(option) = self.options.get(self.options_selected) {
                    self.window_input = Some(WindowInput {
                        model: option.model.clone(),
                        editor: LineEditor::new(String::new()),
                    });
                }
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_project_key(&mut self, key: KeyEvent) -> Action {
        let Some(mut input) = self.project_input.take() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Enter => {
                let name =
                    crate::core::clean_text_fact(input.editor.buffer.trim(), MAX_PROJECT_CHARS);
                let result = if name.is_empty() {
                    self.store.clear_project_dir(&input.dir)
                } else {
                    self.store.set_project_dir(&input.dir, &name)
                };
                match result {
                    Ok(()) => {
                        let assigned = (!name.is_empty()).then(|| name.clone());
                        for row in self.rows.iter_mut().filter(|r| {
                            r.session
                                .cwd
                                .as_deref()
                                .is_some_and(|cwd| cwd.starts_with(&input.dir))
                        }) {
                            row.project.clone_from(&assigned);
                        }
                        self.refresh_scope_project();
                        self.status = Some(if name.is_empty() {
                            format!("project cleared for {}", input.dir.display())
                        } else {
                            format!("{} assigned to project {name}", input.dir.display())
                        });
                    }
                    Err(err) => self.status = Some(format!("error: {err:#}")),
                }
                Action::Refresh
            }
            KeyCode::Esc => Action::None,
            _ => {
                input.editor.handle(key);
                self.project_input = Some(input);
                Action::None
            }
        }
    }

    fn refresh_scope_project(&mut self) {
        self.scope_project = self.repo.as_ref().and_then(|ctx| {
            self.store
                .project_dirs()
                .ok()
                .and_then(|dirs| crate::core::snapshot::project_for(&dirs, &ctx.root))
        });
    }

    fn handle_rename_key(&mut self, key: KeyEvent) -> Action {
        let Some(mut input) = self.rename_input.take() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Enter => {
                let title = input.editor.buffer.trim().to_string();
                if title.is_empty() {
                    self.status = Some("title unchanged".to_string());
                    return Action::None;
                }
                let result = crate::store::upsert_session_facts(
                    self.store.conn(),
                    input.key.tool,
                    &input.key.id,
                    None,
                    Some((title.as_str(), crate::core::TitleKind::Custom)),
                    None,
                    None,
                    None,
                    None,
                    "rename",
                );
                match result {
                    Ok(()) => {
                        if let Some(row) = self.rows.iter_mut().find(|r| r.session.key == input.key)
                        {
                            row.session.title = Some(title);
                        }
                        self.status = Some("session renamed".to_string());
                    }
                    Err(err) => self.status = Some(format!("error: {err:#}")),
                }
                Action::Refresh
            }
            KeyCode::Esc => Action::None,
            _ => {
                input.editor.handle(key);
                self.rename_input = Some(input);
                Action::None
            }
        }
    }

    fn handle_window_input_key(&mut self, key: KeyEvent) -> Action {
        let Some(mut input) = self.window_input.take() else {
            return Action::None;
        };
        match key.code {
            KeyCode::Enter => {
                if input.editor.buffer.trim().is_empty() {
                    match self.store.clear_context_window_override(&input.model) {
                        Ok(()) => {
                            self.status = Some(format!(
                                "context window override for {} cleared",
                                input.model
                            ));
                            if self.screen == Screen::Options {
                                self.refresh_options();
                            }
                        }
                        Err(err) => self.status = Some(format!("error: {err:#}")),
                    }
                    Action::Refresh
                } else if let Some(tokens) = crate::core::parse_token_count(&input.editor.buffer) {
                    match self.store.set_context_window_override(&input.model, tokens) {
                        Ok(()) => {
                            for row in self.rows.iter_mut().filter(|r| {
                                r.session.model.as_deref() == Some(&input.model)
                                    && r.context_window.is_none()
                            }) {
                                row.context_window = Some(tokens);
                            }
                            self.status = Some(format!(
                                "context window for {} set to {}",
                                input.model,
                                crate::core::format_tokens(tokens)
                            ));
                            if self.screen == Screen::Options {
                                self.refresh_options();
                            }
                        }
                        Err(err) => self.status = Some(format!("error: {err:#}")),
                    }
                    Action::Refresh
                } else {
                    self.status = Some(format!(
                        "cannot parse {:?} — use a number like 200000, 200K or 1M",
                        input.editor.buffer
                    ));
                    Action::None
                }
            }
            KeyCode::Esc => Action::None,
            _ => {
                input.editor.handle(key);
                self.window_input = Some(input);
                Action::None
            }
        }
    }

    fn handle_pending_key(&mut self, key: KeyEvent) -> Action {
        match self.pending.take() {
            Some(Pending::NewSession { tools, selected }) => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => Action::None,
                KeyCode::Down => {
                    let selected = (selected + 1).min(tools.len() - 1);
                    self.pending = Some(Pending::NewSession { tools, selected });
                    Action::None
                }
                KeyCode::Up => {
                    let selected = selected.saturating_sub(1);
                    self.pending = Some(Pending::NewSession { tools, selected });
                    Action::None
                }
                KeyCode::Enter => {
                    let tool = tools[selected];
                    self.launch_new_session(tool)
                }
                KeyCode::Char(c @ '1'..='9') => {
                    let index = (c as usize) - ('1' as usize);
                    if let Some(&tool) = tools.get(index) {
                        self.launch_new_session(tool)
                    } else {
                        self.pending = Some(Pending::NewSession { tools, selected });
                        Action::None
                    }
                }
                _ => {
                    self.pending = Some(Pending::NewSession { tools, selected });
                    Action::None
                }
            },
            None => Action::None,
        }
    }

    fn launch_new_session(&mut self, tool: ToolId) -> Action {
        let cwd = self.new_session_cwd();
        let Some(adapter) = adapters::by_id(tool) else {
            return Action::None;
        };
        let spec = adapter.new_session_command(&cwd);
        if !spec.cwd.exists() {
            self.status = Some(format!(
                "cannot start session: directory {} does not exist",
                spec.cwd.display()
            ));
            return Action::None;
        }
        Action::Launch {
            spec,
            replace: true,
        }
    }

    fn new_session_cwd(&self) -> PathBuf {
        if let Some(ctx) = &self.repo
            && self.screen == Screen::Repo
        {
            return ctx.root.clone();
        }
        std::env::current_dir().unwrap_or_else(|_| crate::core::fallback_cwd())
    }

    fn return_screen(&self, leaving: Screen) -> Screen {
        if self.prev_screen == leaving {
            Screen::Monitor
        } else {
            self.prev_screen
        }
    }

    fn cycle_tool_filter(&mut self, direction: i64) {
        let installed: Vec<ToolId> = adapters::installed().iter().map(|a| a.id()).collect();
        let tools = if installed.is_empty() {
            vec![ToolId::Claude, ToolId::Codex, ToolId::Kimi]
        } else {
            installed
        };
        let mut cycle: Vec<Option<ToolId>> = vec![None];
        cycle.extend(tools.into_iter().map(Some));
        let position = cycle
            .iter()
            .position(|t| *t == self.tool_filter)
            .unwrap_or(0);
        let len = cycle.len() as i64;
        let next = (position as i64 + direction).rem_euclid(len) as usize;
        self.tool_filter = cycle[next];
        self.selected = 0;
        self.remember_selection();
    }

    fn toggle_archive(&mut self) {
        if self.screen == Screen::Archive {
            self.screen = self.return_screen(Screen::Archive);
        } else {
            self.prev_screen = self.screen;
            self.screen = Screen::Archive;
        }
        self.selected = 0;
        self.remember_selection();
    }

    fn unmark_selected(&mut self) {
        let Some(row) = self.selected_row() else {
            return;
        };
        if row.demo_archived {
            self.status = Some(
                "this session is archived by the demo config — set archived to false there"
                    .to_string(),
            );
            return;
        }
        let session = row.session.clone();
        let native = session.native_archived;
        if native && !self.unarchive_natively(&session) {
            return;
        }
        match self.store.reopen(session.key.tool, &session.key.id) {
            Ok(_) => {
                if let Some(row) = self.rows.iter_mut().find(|r| r.session.key == session.key) {
                    row.session.native_archived = false;
                    row.completed = false;
                    row.completed_at = None;
                }
                self.status = Some(if native {
                    format!("session unarchived in {}", session.key.tool.display_name())
                } else {
                    "session reopened".to_string()
                });
            }
            Err(err) => self.status = Some(format!("error: {err:#}")),
        }
    }

    fn unarchive_natively(&mut self, session: &crate::core::SessionSummary) -> bool {
        let Some(adapter) = adapters::by_id(session.key.tool) else {
            self.status = Some(format!(
                "this session is archived inside {} itself — unarchive it there",
                session.key.tool.display_name()
            ));
            return false;
        };
        match adapter.unarchive_session(session) {
            Ok(()) => true,
            Err(err) => {
                self.status = Some(format!("{err:#}"));
                false
            }
        }
    }

    fn launch_selected(&mut self, replace: bool) -> Action {
        let Some(row) = self.selected_row() else {
            return Action::None;
        };
        let Some(adapter) = adapters::by_id(row.session.key.tool) else {
            return Action::None;
        };
        if row.session.native_archived && adapter.resume_needs_unarchive() {
            self.status = Some(format!(
                "this session is archived inside {} — press u to unarchive it first",
                row.session.key.tool.display_name()
            ));
            return Action::None;
        }
        let spec = adapter.resume_command(&row.session);
        if !spec.cwd.exists() {
            self.status = Some(format!(
                "cannot resume: directory {} no longer exists",
                spec.cwd.display()
            ));
            return Action::None;
        }
        Action::Launch { spec, replace }
    }

    fn move_selection(&mut self, delta: i64) {
        let len = self.visible_indices().len();
        if len == 0 {
            return;
        }
        let next = (self.selected as i64 + delta).clamp(0, len as i64 - 1);
        self.selected = next as usize;
        self.remember_selection();
    }

    fn remember_selection(&mut self) {
        let visible = self.visible_indices();
        self.selected_key = visible
            .get(self.selected)
            .map(|&i| self.rows[i].session.key.clone());
    }
}

const MAX_TREE_DEPTH: usize = 64;

pub(crate) fn tree_components(cwd: Option<&std::path::Path>) -> Vec<String> {
    let Some(cwd) = cwd else {
        return vec!["?".to_string()];
    };
    let (root, rest) = match dirs::home_dir().and_then(|home| {
        cwd.strip_prefix(&home)
            .ok()
            .map(std::path::Path::to_path_buf)
    }) {
        Some(rest) => ("~".to_string(), rest),
        None => ("/".to_string(), cwd.components().skip(1).collect()),
    };
    let mut components = vec![root];
    components.extend(
        rest.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if components.len() > MAX_TREE_DEPTH {
        let tail = components.split_off(MAX_TREE_DEPTH - 1);
        components.push(format!("…/{}", tail.last().unwrap()));
    }
    components
}

pub(crate) fn row_tree_components(row: &SessionRow) -> Vec<String> {
    row.project_alias().map_or_else(
        || tree_components(row.session.cwd.as_deref()),
        |alias| {
            let path = std::path::Path::new(alias);
            if path.is_absolute() {
                tree_components(Some(path))
            } else {
                vec![alias.to_string()]
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{Liveness, SessionSummary};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn fake_row(id: &str) -> SessionRow {
        SessionRow {
            project_alias: None,
            project: None,
            demo_archived: false,
            session: SessionSummary {
                key: SessionKey {
                    tool: ToolId::Claude,
                    id: id.to_string(),
                },
                cwd: None,
                title: Some(format!("session {id}")),
                preview: None,
                model: None,
                origin_url: None,
                created_at_ms: None,
                updated_at_ms: 0,
                native_archived: false,
                native_tokens_used: None,
                usage_files: Vec::new(),
            },
            liveness: Liveness::Idle,
            completed: false,
            completed_at: None,
            usage: None,
            context_tokens: None,
            context_window: None,
        }
    }

    fn test_app_with(name: &str, repo: Option<RepoContext>, repo_forced: bool) -> App {
        let dir = PathBuf::from(".tmp/fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join(format!("app-{name}.db"));
        let _ = std::fs::remove_file(&db);
        let store = Store::open(Some(&db)).unwrap();
        let mut app = App::new(store, repo, repo_forced);
        app.rows = vec![fake_row("a"), fake_row("b"), fake_row("c")];
        app.received_first_snapshot = true;
        app
    }

    fn test_app(name: &str) -> App {
        test_app_with(name, None, false)
    }

    fn unmatched_repo() -> RepoContext {
        RepoContext {
            root: PathBuf::from("/definitely/not/matching"),
            name: "repo".to_string(),
            worktree_roots: Vec::new(),
            origin_url: None,
            is_git: true,
        }
    }

    #[test]
    fn c_enters_marking_mode_and_enter_completes_marked() {
        let mut app = test_app("multi");
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.marking, Some(Marking::Complete));
        assert_eq!(app.marked.len(), 1);
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.marked.len(), 2);
        app.handle_key(key(KeyCode::Enter));
        assert!(app.marking.is_none());
        assert!(app.marked.is_empty());
        assert_eq!(app.rows.iter().filter(|r| r.completed).count(), 2);
        assert_eq!(app.visible_indices().len(), 1);
    }

    #[test]
    fn esc_cancels_marking_mode_without_completing() {
        let mut app = test_app("esc-marking");
        app.handle_key(key(KeyCode::Char('c')));
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.marked.len(), 2);
        app.handle_key(key(KeyCode::Esc));
        assert!(app.marking.is_none());
        assert!(app.marked.is_empty());
        assert_eq!(app.rows.iter().filter(|r| r.completed).count(), 0);
    }

    #[test]
    fn esc_leaves_archive() {
        let mut app = test_app("esc");
        app.handle_key(key(KeyCode::Char('a')));
        assert_eq!(app.screen, Screen::Archive);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Monitor);
    }

    #[test]
    fn d_in_monitor_marks_for_deletion_directly() {
        let mut app = test_app("delete-monitor");
        let dir = PathBuf::from(".tmp/fixtures/del-live");
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("bbbb.jsonl");
        std::fs::write(&transcript, "{}\n").unwrap();
        app.rows[0].session.usage_files = vec![transcript.clone()];
        assert_eq!(app.screen, Screen::Monitor);
        app.handle_key(key(KeyCode::Char('d')));
        assert_eq!(app.marking, Some(Marking::Delete));
        app.handle_key(key(KeyCode::Enter));
        assert!(!transcript.exists());
        assert_eq!(app.rows.len(), 2);
    }

    #[test]
    fn tree_from_archive_shows_completed_sessions() {
        let mut app = test_app("archive-tree");
        app.rows[0].completed = true;
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('v')));
        assert!(app.tree_of_archive());
        let visible = app.visible_indices();
        assert_eq!(visible.len(), 1);
        assert!(app.rows[visible[0]].completed);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Archive);
    }

    #[test]
    fn v_toggles_tree_view_grouped_by_location() {
        let mut app = test_app("tree");
        app.rows[0].session.cwd = Some(PathBuf::from("/repo/zeta"));
        app.rows[0].session.updated_at_ms = 100;
        app.rows[1].session.cwd = Some(PathBuf::from("/repo/alpha"));
        app.rows[1].session.updated_at_ms = 50;
        app.rows[2].session.cwd = Some(PathBuf::from("/repo/alpha"));
        app.rows[2].session.updated_at_ms = 200;
        app.handle_key(key(KeyCode::Char('v')));
        assert_eq!(app.screen, Screen::Tree);
        let visible = app.visible_indices();
        assert_eq!(
            visible
                .iter()
                .map(|&i| app.rows[i].session.key.id.as_str())
                .collect::<Vec<_>>(),
            vec!["c", "b", "a"]
        );
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Monitor);
    }

    #[test]
    fn tree_view_order_groups_filesystem_roots_together() {
        let mut app = test_app("tree-order");
        app.rows[0].session.cwd = Some(PathBuf::from("/Applications/a"));
        app.rows[1].session.cwd = Some(dirs::home_dir().unwrap().join("b"));
        app.rows[2].session.cwd = Some(PathBuf::from("/opt/c"));
        app.handle_key(key(KeyCode::Char('v')));
        let visible = app.visible_indices();
        assert_eq!(
            visible
                .iter()
                .map(|&i| app.rows[i].session.key.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "c", "b"]
        );
    }

    #[test]
    fn d_marks_and_enter_deletes_session_and_tool_files() {
        let mut app = test_app("delete");
        let dir = PathBuf::from(".tmp/fixtures/del-session");
        std::fs::create_dir_all(&dir).unwrap();
        let transcript = dir.join("aaaa.jsonl");
        std::fs::write(&transcript, "{}\n").unwrap();
        app.rows[0].completed = true;
        app.rows[0].session.usage_files = vec![transcript.clone()];
        app.handle_key(key(KeyCode::Char('a')));
        assert_eq!(app.visible_indices().len(), 1);
        app.handle_key(key(KeyCode::Char('d')));
        assert_eq!(app.marking, Some(Marking::Delete));
        assert_eq!(app.marked.len(), 1);
        app.handle_key(key(KeyCode::Enter));
        assert!(!transcript.exists());
        assert_eq!(app.rows.len(), 2);
        assert_eq!(app.visible_indices().len(), 0);
    }

    #[test]
    fn esc_aborts_delete_marking() {
        let mut app = test_app("delete-abort");
        app.rows[0].completed = true;
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('d')));
        assert_eq!(app.marking, Some(Marking::Delete));
        app.handle_key(key(KeyCode::Esc));
        assert!(app.marking.is_none());
        assert_eq!(app.rows.len(), 3);
    }

    #[test]
    fn delete_skips_sessions_that_are_still_open() {
        let mut app = test_app("delete-open");
        app.rows[0].completed = true;
        app.rows[0].liveness = Liveness::Running;
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('d')));
        app.handle_key(key(KeyCode::Enter));
        assert!(app.status.as_deref().unwrap().contains("skipped"));
        assert_eq!(app.rows.len(), 3);
    }

    #[test]
    fn options_screen_sets_persistent_context_window() {
        let mut app = test_app("window");
        app.rows[0].session.model = Some("claude-fable-5".to_string());
        app.rows[1].session.model = Some("claude-fable-5".to_string());
        app.handle_key(key(KeyCode::Char('o')));
        assert_eq!(app.screen, Screen::Options);
        assert_eq!(app.options.len(), 1);
        app.handle_key(key(KeyCode::Enter));
        assert!(app.window_input.is_some());
        for c in "200K".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.rows[0].context_window, Some(200_000));
        assert_eq!(app.rows[1].context_window, Some(200_000));
        assert_eq!(
            app.store
                .context_window_overrides()
                .unwrap()
                .get("claude-fable-5"),
            Some(&200_000)
        );
        assert!(app.options[0].overridden);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Monitor);
    }

    #[test]
    fn fuzzy_filter_is_live_and_ranked() {
        let mut app = test_app("fuzzy");
        app.rows[0].session.title = Some("fix gesture handler crash".to_string());
        app.rows[1].session.title = Some("gradle handles cache".to_string());
        app.rows[2].session.title = Some("write docs".to_string());
        app.handle_key(key(KeyCode::Char('f')));
        for c in "geshan".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        let visible = app.visible_indices();
        assert_eq!(visible.len(), 1);
        assert_eq!(
            app.rows[visible[0]].session.title.as_deref(),
            Some("fix gesture handler crash")
        );
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.filter.as_deref(), Some("geshan"));
        assert_eq!(app.visible_indices().len(), 1);
    }

    #[test]
    fn fuzzy_scoring_prefers_consecutive_and_word_starts() {
        use crate::core::fuzzy_score;
        assert!(fuzzy_score("abc", "abc").unwrap() > fuzzy_score("abc", "a1b2c3").unwrap());
        assert!(fuzzy_score("na", "react native").unwrap() > fuzzy_score("na", "banana").unwrap());
        assert_eq!(
            fuzzy_score("RE", "React Native"),
            fuzzy_score("re", "react native")
        );
        assert!(fuzzy_score("re", "react native").is_some());
        assert!(fuzzy_score("xyz", "react native").is_none());
        assert!(fuzzy_score("two words", "words and two").is_some());
    }

    #[test]
    fn token_count_parsing() {
        use crate::core::parse_token_count;
        assert_eq!(parse_token_count("200000"), Some(200_000));
        assert_eq!(parse_token_count("200K"), Some(200_000));
        assert_eq!(parse_token_count("1M"), Some(1_000_000));
        assert_eq!(parse_token_count("1.5m"), Some(1_500_000));
        assert_eq!(parse_token_count("bogus"), None);
        assert_eq!(parse_token_count(""), None);
        assert_eq!(parse_token_count("0"), None);
    }

    #[test]
    fn ctrl_c_needs_double_press() {
        let mut app = test_app("ctrlc");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(app.handle_key(ctrl_c), Action::None));
        assert!(app.status.as_deref().unwrap().contains("again"));
        assert!(matches!(app.handle_key(ctrl_c), Action::Quit));
    }

    #[test]
    fn reordered_snapshot_is_held_until_refresh_key() {
        let mut app = test_app("held-update");
        let reordered = vec![fake_row("b"), fake_row("a"), fake_row("c")];
        app.apply_snapshot(reordered);
        assert!(app.pending_snapshot.is_some());
        assert_eq!(app.rows[0].session.key.id, "a");
        assert!(matches!(
            app.handle_key(key(KeyCode::Char('r'))),
            Action::Refresh
        ));
        assert!(app.pending_snapshot.is_none());
        assert_eq!(app.rows[0].session.key.id, "b");
    }

    #[test]
    fn m_renames_session_with_sticky_custom_title() {
        let mut app = test_app("rename");
        app.handle_key(key(KeyCode::Char('m')));
        assert!(app.rename_input.is_some());
        for _ in 0.."session a".len() {
            app.handle_key(key(KeyCode::Backspace));
        }
        for c in "My task".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.rows[0].session.title.as_deref(), Some("My task"));
        let meta = app.store.session_meta_map().unwrap();
        let stored = meta
            .get(&SessionKey {
                tool: ToolId::Claude,
                id: "a".to_string(),
            })
            .unwrap();
        assert_eq!(stored.title.as_deref(), Some("My task"));
        assert_eq!(stored.title_source.as_deref(), Some("custom"));
    }

    #[test]
    fn same_order_snapshot_applies_in_place() {
        let mut app = test_app("live-update");
        let mut refreshed = vec![fake_row("a"), fake_row("b"), fake_row("c")];
        refreshed[0].context_tokens = Some(123);
        app.apply_snapshot(refreshed);
        assert!(app.pending_snapshot.is_none());
        assert_eq!(app.rows[0].context_tokens, Some(123));
    }

    #[test]
    fn refresh_flag_forces_snapshot_apply() {
        let mut app = test_app("forced-update");
        app.apply_next_snapshot = true;
        let reordered = vec![fake_row("c"), fake_row("b"), fake_row("a")];
        app.apply_snapshot(reordered);
        assert!(app.pending_snapshot.is_none());
        assert_eq!(app.rows[0].session.key.id, "c");
    }

    #[test]
    fn q_does_not_quit_but_cancels_marking_mode() {
        let mut app = test_app("quit");
        assert!(matches!(
            app.handle_key(key(KeyCode::Char('q'))),
            Action::None
        ));
        app.handle_key(key(KeyCode::Char('c')));
        assert_eq!(app.marking, Some(Marking::Complete));
        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.marking.is_none());
        assert!(app.marked.is_empty());
        assert_eq!(app.rows.iter().filter(|r| r.completed).count(), 0);
    }

    #[test]
    fn esc_leaves_archive_after_a_tree_round_trip() {
        let mut app = test_app("archive-tree-esc");
        app.rows[0].completed = true;
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Archive);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Monitor);
    }

    #[test]
    fn esc_leaves_tree_after_an_archive_round_trip() {
        let mut app = test_app("tree-archive-esc");
        app.handle_key(key(KeyCode::Char('v')));
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Tree);
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Monitor);
    }

    #[test]
    fn u_on_a_tool_without_unarchive_support_says_where_to_unarchive() {
        let mut app = test_app("native-archive-unsupported");
        app.rows[0].session.key.tool = ToolId::Kimi;
        app.rows[0].session.native_archived = true;
        app.rows[0].completed = true;
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('u')));
        assert!(app.rows[0].session.native_archived);
        assert!(app.rows[0].completed);
        assert_eq!(
            app.status.as_deref(),
            Some("this session is archived inside Kimi Code itself — unarchive it there")
        );
    }

    #[test]
    fn u_asks_the_tool_to_unarchive_a_natively_archived_session() {
        let mut app = test_app("native-archive-delegates");
        app.rows[0].session.key.tool = ToolId::Codex;
        app.rows[0].session.key.id = "-rf".to_string();
        app.rows[0].session.native_archived = true;
        app.rows[0].completed = true;
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('u')));
        assert!(
            app.status
                .as_deref()
                .is_some_and(|s| s.contains("unexpected codex session id")),
            "u must reach the codex adapter, got {:?}",
            app.status
        );
        assert!(app.rows[0].session.native_archived);
        assert!(app.rows[0].completed);
    }

    #[test]
    fn enter_leaves_szpont_and_shift_e_comes_back_to_it() {
        let mut app = test_app("launch-mode");
        match app.handle_key(key(KeyCode::Enter)) {
            Action::Launch { replace, .. } => assert!(replace, "enter must not return to szpont"),
            _ => panic!("enter did not launch"),
        }
        match app.handle_key(KeyEvent::new(KeyCode::Char('E'), KeyModifiers::SHIFT)) {
            Action::Launch { replace, .. } => assert!(!replace, "E must run here and come back"),
            _ => panic!("E did not launch"),
        }
    }

    #[test]
    fn a_new_session_also_leaves_szpont() {
        let mut app = test_app("launch-mode-new");
        let tool = ToolId::Claude;
        match app.launch_new_session(tool) {
            Action::Launch { replace, .. } => {
                assert!(replace, "a new session must not return to szpont");
            }
            _ => panic!("new session did not launch"),
        }
    }

    #[test]
    fn enter_refuses_to_resume_a_natively_archived_session() {
        let mut app = test_app("native-archive-resume");
        app.rows[0].session.key.tool = ToolId::Codex;
        app.rows[0].session.native_archived = true;
        app.rows[0].session.cwd = Some(PathBuf::from("."));
        app.rows[0].completed = true;
        app.handle_key(key(KeyCode::Char('a')));
        let action = app.handle_key(key(KeyCode::Enter));
        assert!(matches!(action, Action::None));
        assert_eq!(
            app.status.as_deref(),
            Some("this session is archived inside Codex — press u to unarchive it first")
        );
    }

    #[test]
    fn u_reopens_in_the_archive_tree_and_c_is_disabled_there() {
        let mut app = test_app("archive-tree-reopen");
        app.rows[0].completed = true;
        app.rows[0].completed_at = Some(1);
        app.handle_key(key(KeyCode::Char('a')));
        app.handle_key(key(KeyCode::Char('v')));
        assert!(app.tree_of_archive());
        assert_eq!(app.visible_indices().len(), 1);
        app.handle_key(key(KeyCode::Char('c')));
        assert!(app.marking.is_none());
        app.handle_key(key(KeyCode::Char('u')));
        assert!(!app.rows[0].completed);
        assert!(app.rows[0].completed_at.is_none());
    }

    #[test]
    fn refresh_keeps_the_cursor_on_the_selected_session() {
        let mut app = test_app("cursor-follow");
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_key.as_ref().map(|k| k.id.as_str()), Some("b"));
        app.apply_next_snapshot = true;
        app.apply_snapshot(vec![fake_row("c"), fake_row("b"), fake_row("a")]);
        assert_eq!(app.selected, 1);
        assert_eq!(app.selected_row().unwrap().session.key.id, "b");
    }

    #[test]
    fn selection_falls_back_when_the_selected_session_disappears() {
        let mut app = test_app("cursor-fallback");
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected_key.as_ref().map(|k| k.id.as_str()), Some("c"));
        app.apply_next_snapshot = true;
        app.apply_snapshot(vec![fake_row("a"), fake_row("b")]);
        assert_eq!(app.selected, 1);
        assert_eq!(app.selected_row().unwrap().session.key.id, "b");
    }

    #[test]
    fn rename_survives_a_snapshot_scanned_before_it_landed() {
        let mut app = test_app("rename-race");
        let target = SessionKey {
            tool: ToolId::Claude,
            id: "b".to_string(),
        };
        crate::store::upsert_session_facts(
            app.store.conn(),
            target.tool,
            &target.id,
            None,
            Some(("My name", crate::core::TitleKind::Custom)),
            None,
            None,
            None,
            None,
            "rename",
        )
        .unwrap();
        app.apply_next_snapshot = true;
        app.apply_snapshot(vec![fake_row("a"), fake_row("b")]);
        let renamed = app
            .rows
            .iter()
            .find(|r| r.session.key == target)
            .expect("row present");
        assert_eq!(renamed.session.title.as_deref(), Some("My name"));
        assert_eq!(app.rows[0].session.title.as_deref(), Some("session a"));
    }

    #[test]
    fn line_editor_caps_its_buffer_and_seed() {
        let mut editor = LineEditor::new("x".repeat(MAX_EDITOR_CHARS + 100));
        assert_eq!(editor.buffer.chars().count(), MAX_EDITOR_CHARS);
        assert!(!editor.handle(key(KeyCode::Char('y'))));
        assert_eq!(editor.buffer.chars().count(), MAX_EDITOR_CHARS);
        assert!(editor.handle(key(KeyCode::Backspace)));
        assert!(editor.handle(key(KeyCode::Char('y'))));
    }

    #[test]
    fn line_editor_word_and_line_deletion_shortcuts() {
        let ctrl = |code| KeyEvent::new(code, KeyModifiers::CONTROL);
        let alt = |code| KeyEvent::new(code, KeyModifiers::ALT);
        let cmd = |code| KeyEvent::new(code, KeyModifiers::SUPER);

        let mut editor = LineEditor::new("one two three".to_string());
        assert!(editor.handle(ctrl(KeyCode::Char('w'))));
        assert_eq!(editor.buffer, "one two ");
        assert!(editor.handle(alt(KeyCode::Backspace)));
        assert_eq!(editor.buffer, "one ");
        assert!(editor.handle(ctrl(KeyCode::Char('u'))));
        assert_eq!(editor.buffer, "");
        assert!(!editor.handle(ctrl(KeyCode::Char('u'))));

        let mut editor = LineEditor::new("one two".to_string());
        assert!(editor.handle(cmd(KeyCode::Backspace)));
        assert_eq!(editor.buffer, "");

        let mut editor = LineEditor::new("one two".to_string());
        assert!(editor.handle(ctrl(KeyCode::Backspace)));
        assert_eq!(editor.buffer, "one ");
    }

    #[test]
    fn line_editor_ignores_modified_chars_instead_of_inserting() {
        let mut editor = LineEditor::new("abc".to_string());
        assert!(!editor.handle(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL)));
        assert!(!editor.handle(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::ALT)));
        assert_eq!(editor.buffer, "abc");
    }

    #[test]
    fn line_editor_cursor_spans_do_not_shift_text() {
        let mut editor = LineEditor::new("abc".to_string());
        editor.handle(key(KeyCode::Left));
        editor.handle(key(KeyCode::Left));
        let spans = editor.cursor_spans(Style::new());
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "abc");
        assert_eq!(spans[1].content.as_ref(), "b");
        assert!(spans[1].style.add_modifier.contains(Modifier::REVERSED));
        editor.handle(key(KeyCode::End));
        let spans = editor.cursor_spans(Style::new());
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "abc ");
    }

    #[test]
    fn first_empty_repo_snapshot_falls_back_to_monitor() {
        let mut app = test_app_with("repo-fallback", Some(unmatched_repo()), false);
        app.rows.clear();
        app.received_first_snapshot = false;
        assert_eq!(app.screen, Screen::Repo);
        app.apply_snapshot(vec![fake_row("a")]);
        assert_eq!(app.screen, Screen::Monitor);
        assert!(app.status.as_deref().unwrap().contains("no sessions"));
    }

    #[test]
    fn forced_repo_view_is_not_flipped_by_an_empty_snapshot() {
        let mut app = test_app_with("repo-forced", Some(unmatched_repo()), true);
        app.rows.clear();
        app.received_first_snapshot = false;
        app.apply_snapshot(vec![fake_row("a")]);
        assert_eq!(app.screen, Screen::Repo);
    }

    #[test]
    fn tree_components_depth_is_capped() {
        let deep = PathBuf::from(format!("/{}", vec!["d"; 200].join("/")));
        let components = tree_components(Some(&deep));
        assert!(components.len() <= MAX_TREE_DEPTH);
        assert_eq!(components.last().unwrap(), "…/d");
    }

    #[test]
    fn allowlisted_rows_search_and_group_by_alias_without_real_path() {
        let mut app = test_app("presentation-alias");
        app.rows.truncate(1);
        app.rows[0].session.cwd = Some(PathBuf::from("/private/secret-project"));
        app.rows[0].project_alias = Some("Demo Project".to_string());
        assert_eq!(row_tree_components(&app.rows[0]), vec!["Demo Project"]);

        app.filter = Some("secret-project".to_string());
        assert_eq!(app.visible_indices(), Vec::<usize>::new());
        app.filter = Some("Demo Project".to_string());
        assert_eq!(app.visible_indices(), vec![0]);

        let home = dirs::home_dir().unwrap();
        app.rows[0].project_alias = Some(home.join("demo/project").display().to_string());
        assert_eq!(
            row_tree_components(&app.rows[0]),
            vec!["~", "demo", "project"]
        );
    }

    #[test]
    fn assigning_a_project_persists_and_labels_every_matching_row() {
        let mut app = test_app("project-assign");
        let dir = std::path::PathBuf::from("/tmp/alpha");
        app.rows = vec![fake_row("a"), fake_row("b")];
        app.rows[0].session.cwd = Some(dir.join("child"));
        app.rows[1].session.cwd = Some(std::path::PathBuf::from("/tmp/other"));
        app.selected = 0;
        app.remember_selection();

        app.handle_key(key(KeyCode::Char('s')));
        assert!(
            app.project_input.is_some(),
            "s did not open the project prompt"
        );
        for c in "dboard".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        app.handle_key(key(KeyCode::Enter));

        assert!(
            app.project_input.is_none(),
            "prompt stayed open after enter"
        );
        assert_eq!(app.rows[0].project.as_deref(), Some("dboard"));
        assert_eq!(app.rows[1].project, None, "unrelated row was labelled");
        let stored = app.store.project_dirs().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].1, "dboard");
    }

    #[test]
    fn two_directories_can_share_one_project_and_scope_prefers_it() {
        let mut app = test_app("project-scope");
        let alpha = std::path::PathBuf::from("/tmp/alpha");
        let beta = std::path::PathBuf::from("/tmp/beta");
        app.store.set_project_dir(&alpha, "dboard").unwrap();
        app.store.set_project_dir(&beta, "dboard").unwrap();

        let dirs = app.store.project_dirs().unwrap();
        assert_eq!(
            crate::core::snapshot::project_for(&dirs, &alpha.join("deep/child")).as_deref(),
            Some("dboard")
        );
        assert_eq!(
            crate::core::snapshot::project_for(&dirs, &beta).as_deref(),
            Some("dboard")
        );
        assert_eq!(
            crate::core::snapshot::project_for(&dirs, std::path::Path::new("/tmp/gamma")),
            None
        );

        app.repo = Some(crate::core::repo::plain_directory_context(&alpha));
        app.refresh_scope_project();
        assert_eq!(app.scope_project.as_deref(), Some("dboard"));

        app.rows = vec![fake_row("a"), fake_row("b"), fake_row("c")];
        app.rows[0].session.cwd = Some(alpha.clone());
        app.rows[0].project = Some("dboard".to_string());
        app.rows[1].session.cwd = Some(beta.clone());
        app.rows[1].project = Some("dboard".to_string());
        app.rows[2].session.cwd = Some(std::path::PathBuf::from("/tmp/gamma"));
        app.screen = Screen::Repo;

        let visible = app.visible_indices();
        assert_eq!(
            visible.len(),
            2,
            "project scope must span every directory in the project"
        );
        assert!(
            !visible.contains(&2),
            "unrelated directory leaked into scope"
        );
    }
}
