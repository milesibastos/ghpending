use chrono::Utc;
use owo_colors::OwoColorize;
use terminal_size::{Width, terminal_size};

use crate::format::{relative_time, truncate_title};
use crate::github::{ItemKind, RepoResult, RepoStatus};

const HEADER_WIDTH: usize = 60;

fn term_width() -> usize {
    terminal_size().map_or(80, |(Width(w), _)| w as usize)
}

fn should_colorize() -> bool {
    std::env::var("NO_COLOR").is_err()
}

/// Apply a color style only when `color` is true; otherwise return the text as-is.
fn paint<'a>(text: &'a str, color: bool, style: impl FnOnce(&'a str) -> String) -> String {
    if color { style(text) } else { text.to_owned() }
}

pub fn render_digest(results: &[RepoResult]) -> String {
    render_inner(results, should_colorize(), term_width())
}

fn render_inner(results: &[RepoResult], color: bool, width: usize) -> String {
    if results.is_empty() {
        return "No repos tracked. Run `ghpending add` to get started.\n".into();
    }

    let now = Utc::now();
    let mut out = String::new();

    for (i, result) in results.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }

        let prefix = "━━ ";
        let suffix = " ";
        let name_len = result.repo.chars().count();
        let used = prefix.chars().count() + name_len + suffix.chars().count();
        let pad = HEADER_WIDTH.saturating_sub(used);
        let dashes = "─".repeat(pad);

        let repo_colored = paint(&result.repo, color, |s| format!("{}", s.bold().cyan()));
        out.push_str(&format!("{prefix}{repo_colored}{suffix}{dashes}\n"));

        match &result.status {
            RepoStatus::NotFound => {
                let msg = paint("(not found or no access)", color, |s| {
                    format!("{}", s.dimmed())
                });
                out.push_str(&format!("  {msg}\n"));
            }
            RepoStatus::Error(e) => {
                let msg = paint(&format!("(error: {e})"), color, |s| {
                    format!("{}", s.red().dimmed())
                });
                out.push_str(&format!("  {msg}\n"));
            }
            RepoStatus::Items(items) if items.is_empty() => {
                let msg = paint("(nothing pending)", color, |s| format!("{}", s.dimmed()));
                out.push_str(&format!("  {msg}\n"));
            }
            RepoStatus::Items(items) => {
                let title_max = if width > 20 { width - 20 } else { 10 };

                for item in items {
                    let (kind_str, number_str, title_str) = match item.kind {
                        ItemKind::PullRequest => {
                            let ks = paint("PR ", color, |s| format!("{}", s.magenta()));
                            let ns = format!("#{}", item.number);
                            let title = truncate_title(&item.title, title_max);
                            (ks, ns, title)
                        }
                        ItemKind::Issue => {
                            let ks = paint("ISS", color, |s| format!("{}", s.yellow()));
                            let ns = format!("#{}", item.number);
                            let title = truncate_title(&item.title, title_max);
                            (ks, ns, title)
                        }
                    };

                    out.push_str(&format!("  {kind_str}  {number_str}  {title_str}\n"));

                    let rel = relative_time(&item.created_at, &now);
                    let meta = format!("opened {} ago by {}", rel, item.author);
                    let meta_colored = paint(&meta, color, |s| format!("{}", s.dimmed()));
                    out.push_str(&format!("        {meta_colored}\n"));
                }
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{ItemKind, RepoItem, RepoResult, RepoStatus};

    fn make_item(kind: ItemKind, number: u64, title: &str, days_ago: i64) -> RepoItem {
        let created_at = Utc::now() - chrono::Duration::days(days_ago);
        RepoItem {
            kind,
            number,
            title: title.into(),
            created_at,
            author: "testuser".into(),
        }
    }

    #[test]
    fn no_repos_tracked() {
        let out = render_inner(&[], false, 80);
        assert!(out.contains("ghpending add"));
    }

    #[test]
    fn repo_with_zero_items() {
        let results = vec![RepoResult {
            repo: "owner/empty".into(),
            status: RepoStatus::Items(vec![]),
        }];
        let out = render_inner(&results, false, 80);
        assert!(out.contains("owner/empty"));
        assert!(out.contains("(nothing pending)"));
    }

    #[test]
    fn repo_not_found() {
        let results = vec![RepoResult {
            repo: "owner/missing".into(),
            status: RepoStatus::NotFound,
        }];
        let out = render_inner(&results, false, 80);
        assert!(out.contains("owner/missing"));
        assert!(out.contains("not found or no access"));
    }

    #[test]
    fn repo_error_renders_message() {
        let results = vec![RepoResult {
            repo: "owner/flaky".into(),
            status: RepoStatus::Error("rate limited".into()),
        }];
        let out = render_inner(&results, false, 80);
        assert!(out.contains("owner/flaky"));
        assert!(out.contains("error:"));
        assert!(out.contains("rate limited"));
    }

    #[test]
    fn normal_items_rendered() {
        let results = vec![RepoResult {
            repo: "ratatui-org/ratatui".into(),
            status: RepoStatus::Items(vec![
                make_item(ItemKind::PullRequest, 1842, "Fix overflow in Table", 2),
                make_item(ItemKind::Issue, 1840, "Crash on empty Paragraph", 0),
            ]),
        }];
        let out = render_inner(&results, false, 80);
        assert!(out.contains("ratatui-org/ratatui"));
        assert!(out.contains("PR"));
        assert!(out.contains("#1842"));
        assert!(out.contains("Fix overflow in Table"));
        assert!(out.contains("ISS"));
        assert!(out.contains("#1840"));
        assert!(out.contains("testuser"));
    }

    #[test]
    fn header_uses_separator_dashes() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![]),
        }];
        let out = render_inner(&results, false, 80);
        let first_line = out.lines().next().unwrap();
        assert!(first_line.contains("━━"));
        assert!(first_line.contains("─"));
    }

    #[test]
    fn two_repos_separated_by_blank_line() {
        let results = vec![
            RepoResult {
                repo: "a/b".into(),
                status: RepoStatus::Items(vec![]),
            },
            RepoResult {
                repo: "c/d".into(),
                status: RepoStatus::Items(vec![]),
            },
        ];
        let out = render_inner(&results, false, 80);
        assert!(out.contains("\n\n"));
    }
}
