#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Theme {
    Dark,
    Light,
    Paper,
}

impl Theme {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::Paper => "paper",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
            Theme::Paper => "Paper",
        }
    }

    pub(crate) fn from_str(s: &str) -> Option<Self> {
        match s {
            "dark" => Some(Theme::Dark),
            "light" => Some(Theme::Light),
            "paper" => Some(Theme::Paper),
            _ => None,
        }
    }
}
