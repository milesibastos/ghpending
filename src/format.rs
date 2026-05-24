use chrono::{DateTime, Utc};

pub fn relative_time(then: &DateTime<Utc>, now: &DateTime<Utc>) -> String {
    let secs = (now.signed_duration_since(*then)).num_seconds().max(0);
    if secs < 60 {
        return "just now".into();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h", hours);
    }
    let days = hours / 24;
    if days < 7 {
        return format!("{}d", days);
    }
    let weeks = days / 7;
    if weeks < 4 {
        return format!("{}w", weeks);
    }
    let months = days / 30;
    if months < 12 {
        return format!("{}mo", months);
    }
    let years = days / 365;
    format!("{}y", years)
}

pub fn truncate_title(title: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let chars: Vec<char> = title.chars().collect();
    if chars.len() <= max_width {
        return title.to_owned();
    }
    if max_width <= 1 {
        return "\u{2026}".to_owned();
    }
    let mut s: String = chars[..max_width - 1].iter().collect();
    s.push('\u{2026}');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(y: i32, mo: u32, d: u32, h: u32, m: u32, s: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, m, s).unwrap()
    }

    fn now() -> DateTime<Utc> {
        dt(2026, 5, 23, 12, 0, 0)
    }

    #[test]
    fn test_just_now() {
        let then = dt(2026, 5, 23, 11, 59, 30);
        assert_eq!(relative_time(&then, &now()), "just now");
    }

    #[test]
    fn test_minutes() {
        let then = dt(2026, 5, 23, 11, 55, 0);
        assert_eq!(relative_time(&then, &now()), "5m");
    }

    #[test]
    fn test_hours() {
        let then = dt(2026, 5, 23, 9, 0, 0);
        assert_eq!(relative_time(&then, &now()), "3h");
    }

    #[test]
    fn test_days() {
        let then = dt(2026, 5, 21, 12, 0, 0);
        assert_eq!(relative_time(&then, &now()), "2d");
    }

    #[test]
    fn test_weeks() {
        let then = dt(2026, 5, 9, 12, 0, 0);
        assert_eq!(relative_time(&then, &now()), "2w");
    }

    #[test]
    fn test_months() {
        // 2026-02-23 to 2026-05-23 = 89 days → 89/30 = 2mo
        let then = dt(2026, 2, 23, 12, 0, 0);
        assert_eq!(relative_time(&then, &now()), "2mo");
    }

    #[test]
    fn test_years() {
        let then = dt(2024, 5, 23, 12, 0, 0);
        assert_eq!(relative_time(&then, &now()), "2y");
    }

    #[test]
    fn truncate_short_unchanged() {
        assert_eq!(truncate_title("hello", 20), "hello");
    }

    #[test]
    fn truncate_exact_unchanged() {
        assert_eq!(truncate_title("hello", 5), "hello");
    }

    #[test]
    fn truncate_long_gets_ellipsis() {
        let result = truncate_title("Fix overflow in Table widget", 15);
        assert!(result.ends_with('\u{2026}'));
        assert_eq!(result.chars().count(), 15);
    }

    #[test]
    fn truncate_width_zero() {
        assert_eq!(truncate_title("anything", 0), "");
    }

    #[test]
    fn truncate_width_one() {
        assert_eq!(truncate_title("anything", 1), "\u{2026}");
    }
}
