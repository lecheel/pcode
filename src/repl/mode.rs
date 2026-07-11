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
    GitDiff,
    FilePicker,
}
impl Mode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "LLM",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "VISUAL-LINE",
            Mode::Merge => "MERGE",
            Mode::GitLog => "GITLOG",
            Mode::GitDiff => "GDIFF",
            Mode::FilePicker => "FILEPICKER",
        }
    }
    pub fn status_color(&self) -> crossterm::style::Color {
        match self {
            Mode::Normal => crossterm::style::Color::Cyan,
            Mode::Insert => crossterm::style::Color::Green,
            Mode::Command => crossterm::style::Color::Yellow,
            Mode::Search => crossterm::style::Color::Yellow,
            Mode::Visual | Mode::VisualLine => crossterm::style::Color::Magenta,
            Mode::Merge => crossterm::style::Color::Blue,
            Mode::GitLog => crossterm::style::Color::Magenta,
            Mode::GitDiff => crossterm::style::Color::Magenta,
            Mode::FilePicker => crossterm::style::Color::Cyan,
        }
    }
}
