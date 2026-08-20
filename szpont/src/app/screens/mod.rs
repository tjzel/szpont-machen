pub(super) mod monitor;
mod options;

use ratatui::Frame;

use super::{App, Screen};

pub fn draw(frame: &mut Frame, app: &mut App) {
    match app.screen {
        Screen::Monitor | Screen::Repo | Screen::Archive | Screen::Tree => {
            monitor::draw(frame, app);
        }
        Screen::Options => options::draw(frame, app),
    }
}
