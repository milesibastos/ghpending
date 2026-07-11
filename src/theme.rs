use owo_colors::{Style, XtermColors};

pub const THEME_NAMES: &[&str] = &["default", "nerv"];

pub struct Theme {
    pub repo: Style,
    pub pr: Style,
    pub issue: Style,
    pub meta: Style,
    pub error: Style,
}

impl Theme {
    pub fn default_theme() -> Self {
        Theme {
            repo: Style::new().bold().cyan(),
            pr: Style::new().magenta(),
            issue: Style::new().yellow(),
            meta: Style::new().dimmed(),
            error: Style::new().red().dimmed(),
        }
    }

    pub fn nerv() -> Self {
        Theme {
            repo: Style::new().bold().color(XtermColors::LighterHeliotrope),
            pr: Style::new().color(XtermColors::ChartreuseGreen),
            issue: Style::new().color(XtermColors::FlushOrange),
            meta: Style::new().color(XtermColors::WildBlueYonder),
            error: Style::new().color(XtermColors::Red),
        }
    }

    pub fn by_name(name: &str) -> Option<Theme> {
        match name {
            "default" => Some(Theme::default_theme()),
            "nerv" => Some(Theme::nerv()),
            _ => None,
        }
    }
}
