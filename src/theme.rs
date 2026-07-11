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

/// Picks the theme name following the tclock widget contract:
/// `--theme` flag, then `GHPENDING_THEME`, then the generic
/// `TCLOCK_WIDGET_THEME` set by tclock for widget subprocesses, then the
/// config file, then "default". Env vars carrying an unknown name are
/// skipped (with a stderr warning) rather than aborting, so tclock can
/// cycle through palettes ghpending doesn't define.
pub fn resolve_name(
    flag: Option<&str>,
    env_specific: Option<&str>,
    env_generic: Option<&str>,
    config: Option<&str>,
) -> String {
    if let Some(name) = flag {
        return name.to_owned();
    }
    for (var, value) in [
        ("GHPENDING_THEME", env_specific),
        ("TCLOCK_WIDGET_THEME", env_generic),
    ] {
        if let Some(name) = value.map(str::trim).filter(|n| !n.is_empty()) {
            if THEME_NAMES.contains(&name) {
                return name.to_owned();
            }
            eprintln!("warning: ignoring unknown theme {name:?} from ${var}");
        }
    }
    config.unwrap_or("default").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flag_wins_over_everything() {
        let name = resolve_name(
            Some("nerv"),
            Some("default"),
            Some("default"),
            Some("default"),
        );
        assert_eq!(name, "nerv");
    }

    #[test]
    fn specific_env_beats_generic_env_and_config() {
        let name = resolve_name(None, Some("nerv"), Some("default"), Some("default"));
        assert_eq!(name, "nerv");
    }

    #[test]
    fn generic_env_beats_config() {
        let name = resolve_name(None, None, Some("nerv"), Some("default"));
        assert_eq!(name, "nerv");
    }

    #[test]
    fn config_used_when_no_flag_or_env() {
        let name = resolve_name(None, None, None, Some("nerv"));
        assert_eq!(name, "nerv");
    }

    #[test]
    fn defaults_when_nothing_set() {
        assert_eq!(resolve_name(None, None, None, None), "default");
    }

    #[test]
    fn unknown_env_name_falls_through_to_config() {
        let name = resolve_name(None, None, Some("matrix"), Some("nerv"));
        assert_eq!(name, "nerv");
    }

    #[test]
    fn empty_env_is_ignored() {
        let name = resolve_name(None, Some("  "), Some(""), None);
        assert_eq!(name, "default");
    }

    #[test]
    fn unknown_flag_name_is_passed_through_for_caller_to_reject() {
        assert_eq!(resolve_name(Some("matrix"), None, None, None), "matrix");
    }
}
