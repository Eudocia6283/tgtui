use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Normal,
    Insert,
    FuzzyFind,
    Help,
    ConfirmDelete,
}

impl fmt::Display for AppMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppMode::Normal => write!(f, "NORMAL"),
            AppMode::Insert => write!(f, "INSERT"),
            AppMode::FuzzyFind => write!(f, "FUZZY"),
            AppMode::Help => write!(f, "HELP"),
            AppMode::ConfirmDelete => write!(f, "DELETE?"),
        }
    }
}
