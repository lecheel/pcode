#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Command,
    Search,
    Visual,
    VisualLine,
    Merge,
    GitLog,
}
impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL LINE",
            Mode::Merge => "MERGE",
            Mode::GitLog => "GITLOG",
        }
    }
    pub fn status_color(&self) -> crossterm::style::Color {
        match self {
            Mode::Normal => crossterm::style::Color::Green,
            Mode::Insert => crossterm::style::Color::Cyan,
            Mode::Command => crossterm::style::Color::Yellow,
            Mode::Search => crossterm::style::Color::Magenta,
            Mode::Visual => crossterm::style::Color::Blue,
            Mode::VisualLine => crossterm::style::Color::Blue,
            Mode::Merge => crossterm::style::Color::Magenta,
            Mode::GitLog => crossterm::style::Color::Yellow,
        }
    }
}
