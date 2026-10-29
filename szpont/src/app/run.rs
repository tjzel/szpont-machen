use std::io;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{
    Event, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::{Action, App, AppEvent, Screen, screens};
use crate::cli::Cli;
use crate::core::LaunchSpec;
use crate::scanner::{self, ScannerHandle};
use crate::store::Store;

pub fn run(cli: &Cli) -> anyhow::Result<()> {
    let allowlist = cli
        .session_allowlist
        .as_deref()
        .map(crate::allowlist::SessionAllowlist::load)
        .transpose()?;
    let store = Store::open(cli.db.as_deref())?;
    let start = resolve_repo_context(cli);
    let (event_tx, event_rx) = channel();
    let scanner = scanner::spawn(
        cli.db.clone(),
        cli.refresh_secs,
        cli.no_watch,
        allowlist.clone(),
        event_tx,
    );
    install_panic_hook();
    let mut terminal = setup_terminal()?;
    let mut app = App::new(store, start.ctx, start.forced);
    if start.global {
        app.screen = Screen::Monitor;
        app.prev_screen = Screen::Monitor;
    }
    if let Some(allowlist) = &allowlist {
        app.allowlist_mode = true;
        app.limits_disabled = true;
        app.status = Some(format!(
            "promo allowlist: {} enabled sessions",
            allowlist.len()
        ));
    }
    let result = event_loop(&mut terminal, &mut app, &event_rx, &scanner);
    restore_terminal(&mut terminal)?;
    scanner.quit();
    result
}

struct RepoStart {
    ctx: Option<crate::core::repo::RepoContext>,
    forced: bool,
    global: bool,
}

fn resolve_repo_context(cli: &Cli) -> RepoStart {
    if let Some(path) = &cli.repo {
        let ctx = crate::core::repo::detect(path)
            .unwrap_or_else(|| crate::core::repo::plain_directory_context(path));
        return RepoStart {
            ctx: Some(ctx),
            forced: true,
            global: false,
        };
    }
    let cwd = std::env::current_dir().ok();
    let detected = cwd.as_deref().and_then(crate::core::repo::detect);
    let in_repo = detected.is_some();
    let ctx = detected.or_else(|| {
        cwd.as_deref()
            .map(crate::core::repo::plain_directory_context)
    });
    RepoStart {
        ctx,
        forced: false,
        global: cli.global || !in_repo,
    }
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    event_rx: &Receiver<AppEvent>,
    scanner: &ScannerHandle,
) -> anyhow::Result<()> {
    loop {
        terminal.draw(|frame| screens::draw(frame, app))?;
        if crossterm::event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = crossterm::event::read()?
            && key.kind == KeyEventKind::Press
        {
            match app.handle_key(key) {
                Action::Quit => return Ok(()),
                Action::Refresh => {
                    app.apply_next_snapshot = true;
                    scanner.refresh();
                }
                Action::Launch { spec, replace } => {
                    let outcome = match launch_style(replace, in_tmux()) {
                        LaunchStyle::Replace => {
                            restore_terminal(terminal)?;
                            scanner.quit();
                            return exec_replace(&spec);
                        }
                        LaunchStyle::TmuxWindow => match tmux_open_window(&spec) {
                            Ok(()) => format!("{} opened in a new tmux window", spec.program),
                            Err(err) => {
                                crate::logging::warn(&format!("tmux new-window failed: {err:#}"));
                                suspend_and_run(terminal, &spec)
                            }
                        },
                        LaunchStyle::Here => suspend_and_run(terminal, &spec),
                    };
                    app.status = Some(outcome);
                    app.apply_next_snapshot = true;
                    scanner.refresh();
                }
                Action::None => {}
            }
        }
        while let Ok(event) = event_rx.try_recv() {
            match event {
                AppEvent::Snapshot(rows) => app.apply_snapshot(rows),
                AppEvent::Limits(limits) => app.limits = limits,
                AppEvent::Progress(message) => {
                    app.scan_status = Some(message);
                    app.scanning = true;
                }
                AppEvent::ScanDone(message) => {
                    app.scan_status = Some(message);
                    app.scanning = false;
                }
                AppEvent::ScanError(message) => {
                    app.status = Some(message);
                    app.scanning = false;
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LaunchStyle {
    Replace,
    TmuxWindow,
    Here,
}

fn launch_style(replace: bool, in_tmux: bool) -> LaunchStyle {
    match (replace, in_tmux) {
        (true, true) => LaunchStyle::TmuxWindow,
        (true, false) => LaunchStyle::Replace,
        (false, _) => LaunchStyle::Here,
    }
}

fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some_and(|value| !value.is_empty())
}

fn tmux_open_window(spec: &LaunchSpec) -> anyhow::Result<()> {
    let mut command = Command::new("tmux");
    command.args(tmux_new_window_args(spec));
    if let Some(path) = sanitized_path_env() {
        command.env("PATH", path);
    }
    let output = command.output()?;
    if !output.status.success() {
        let stderr = crate::core::sanitize_for_terminal(&String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("tmux exited with {}: {}", output.status, stderr.trim());
    }
    Ok(())
}

fn tmux_new_window_args(spec: &LaunchSpec) -> Vec<std::ffi::OsString> {
    let name = std::path::Path::new(&spec.program).file_name().map_or_else(
        || spec.program.clone(),
        |n| n.to_string_lossy().into_owned(),
    );
    let shell_command = std::iter::once(&spec.program)
        .chain(spec.args.iter())
        .map(|arg| shell_escape(arg))
        .collect::<Vec<String>>()
        .join(" ");
    vec![
        "new-window".into(),
        "-n".into(),
        name.into(),
        "-c".into(),
        spec.cwd.clone().into_os_string(),
        "--".into(),
        shell_command.into(),
    ]
}

fn shell_escape(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | ':' | '@'))
    {
        return arg.to_string();
    }
    format!("'{}'", arg.replace('\'', "'\\''"))
}

fn suspend_and_run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    spec: &LaunchSpec,
) -> String {
    if let Err(err) = restore_terminal(terminal) {
        return format!("terminal error: {err:#}");
    }
    let status = launch_command(spec).status();
    let outcome = match status {
        Ok(status) if status.success() => format!("{} exited", spec.program),
        Ok(status) => format!("{} exited with {status}", spec.program),
        Err(err) => format!("failed to launch {}: {err}", spec.program),
    };
    if let Err(err) = enter_tui(terminal) {
        return format!("terminal error: {err:#}");
    }
    outcome
}

fn exec_replace(spec: &LaunchSpec) -> anyhow::Result<()> {
    use std::os::unix::process::CommandExt;
    let err = launch_command(spec).exec();
    Err(anyhow::anyhow!("failed to exec {}: {err}", spec.program))
}

fn launch_command(spec: &LaunchSpec) -> Command {
    let mut command = Command::new(&spec.program);
    command.args(&spec.args).current_dir(&spec.cwd);
    if let Some(path) = sanitized_path_env() {
        command.env("PATH", path);
    }
    command
}

fn sanitized_path_env() -> Option<std::ffi::OsString> {
    let paths = std::env::var_os("PATH")?;
    let filtered = std::env::split_paths(&paths)
        .filter(|p| !p.as_os_str().is_empty() && p != std::path::Path::new("."));
    std::env::join_paths(filtered).ok()
}

static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        let _ = execute!(
            stdout,
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        KEYBOARD_ENHANCED.store(true, Ordering::Relaxed);
    }
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    if KEYBOARD_ENHANCED.load(Ordering::Relaxed) {
        let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, cursor::Show)?;
    Ok(())
}

fn enter_tui(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen, cursor::Hide)?;
    if KEYBOARD_ENHANCED.load(Ordering::Relaxed) {
        let _ = execute!(
            terminal.backend_mut(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
    }
    terminal.clear()?;
    Ok(())
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if KEYBOARD_ENHANCED.load(Ordering::Relaxed) {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
        original(info);
    }));
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn enter_replaces_szpont_unless_tmux_can_give_it_a_window() {
        assert_eq!(launch_style(true, false), LaunchStyle::Replace);
        assert_eq!(launch_style(true, true), LaunchStyle::TmuxWindow);
        assert_eq!(launch_style(false, false), LaunchStyle::Here);
        assert_eq!(launch_style(false, true), LaunchStyle::Here);
    }

    #[test]
    fn shell_escape_passes_safe_args_and_quotes_the_rest() {
        assert_eq!(shell_escape("--resume"), "--resume");
        assert_eq!(shell_escape("session_ab12-cd"), "session_ab12-cd");
        assert_eq!(shell_escape("a b"), "'a b'");
        assert_eq!(shell_escape("it's"), "'it'\\''s'");
        assert_eq!(shell_escape(""), "''");
        assert_eq!(shell_escape("$(rm -rf x)"), "'$(rm -rf x)'");
    }

    #[test]
    fn tmux_new_window_args_set_name_cwd_and_escaped_command() {
        let spec = LaunchSpec {
            program: "claude".to_string(),
            args: vec!["--resume".to_string(), "abc 123".to_string()],
            cwd: PathBuf::from("/tmp/my repo"),
        };
        let args = tmux_new_window_args(&spec);
        let args: Vec<String> = args
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "new-window",
                "-n",
                "claude",
                "-c",
                "/tmp/my repo",
                "--",
                "claude --resume 'abc 123'",
            ]
        );
    }
}
