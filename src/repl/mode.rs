#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Search,
}
impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
        }
    }
    pub fn status_color(&self) -> crossterm::style::Color {
        match self {
            Mode::Normal => crossterm::style::Color::Green,
            Mode::Insert => crossterm::style::Color::Cyan,
            Mode::Command => crossterm::style::Color::Yellow,
            Mode::Search => crossterm::style::Color::Magenta,
        }
    }
}
