use chrono::Utc;
use owo_colors::Style;
use terminal_size::{Width, terminal_size};

use crate::format::{relative_time, truncate_title};
use crate::github::{
    CodexReaction, ItemKind, PrExtra, RepoItem, RepoResult, RepoStatus, ReviewDecision, ReviewState,
};
use crate::theme::Theme;

fn term_width() -> usize {
    terminal_size().map_or(80, |(Width(w), _)| w as usize)
}

fn should_colorize() -> bool {
    std::env::var("NO_COLOR").is_err()
}

fn paint(text: &str, color: bool, style: Style) -> String {
    if color {
        style.style(text).to_string()
    } else {
        text.to_owned()
    }
}

pub fn render_digest(results: &[RepoResult], theme: &Theme, filtered: bool) -> String {
    render_inner_filtered(results, theme, should_colorize(), term_width(), filtered)
}

#[cfg(test)]
fn render_inner(results: &[RepoResult], theme: &Theme, color: bool, width: usize) -> String {
    render_inner_filtered(results, theme, color, width, false)
}

fn render_inner_filtered(
    results: &[RepoResult],
    theme: &Theme,
    color: bool,
    width: usize,
    filtered: bool,
) -> String {
    if results.is_empty() {
        return "No repos tracked. Run `ghpending add` to get started.\n".into();
    }

    let now = Utc::now();
    let total = results.len();
    let with_pending = results
        .iter()
        .filter(|r| matches!(&r.status, RepoStatus::Items(items) if !items.is_empty()))
        .count();
    let failures = results
        .iter()
        .filter(|r| matches!(&r.status, RepoStatus::Error(_)))
        .count();

    let mut body = String::new();
    let mut shown = 0;

    for result in results {
        if matches!(&result.status, RepoStatus::Items(items) if items.is_empty()) {
            continue;
        }

        if shown > 0 {
            body.push('\n');
        }
        shown += 1;

        let repo_colored = paint(&result.repo, color, theme.repo);
        body.push_str(&format!("━━ {repo_colored}\n"));

        match &result.status {
            RepoStatus::NotFound => {
                let msg = paint("(not found or no access)", color, theme.meta);
                body.push_str(&format!("  {msg}\n"));
            }
            RepoStatus::Error(e) => {
                let msg = paint(&format!("(error: {e})"), color, theme.error);
                body.push_str(&format!("  {msg}\n"));
            }
            RepoStatus::Items(items) => {
                let title_max = if width > 20 { width - 20 } else { 10 };

                for item in items {
                    let (kind_str, number_str, title_str) = match item.kind {
                        ItemKind::PullRequest => {
                            let ks = paint("PR ", color, theme.pr);
                            let ns = format!("#{}", item.number);
                            let title = truncate_title(&item.title, title_max);
                            (ks, ns, title)
                        }
                        ItemKind::Issue => {
                            let ks = paint("ISS", color, theme.issue);
                            let ns = format!("#{}", item.number);
                            let title = truncate_title(&item.title, title_max);
                            (ks, ns, title)
                        }
                    };

                    body.push_str(&format!("  {kind_str}  {number_str}  {title_str}\n"));

                    let rel = relative_time(&item.created_at, &now);
                    let mut meta = format!("opened {} ago by {}", rel, item.author);
                    if let Some(state) = pr_state_label(item) {
                        meta.push_str(" · ");
                        meta.push_str(state);
                    }
                    let meta_colored = paint(&meta, color, theme.meta);
                    body.push_str(&format!("        {meta_colored}\n"));

                    if let Some(line) = pr_detail_line(item) {
                        let line_colored = paint(&line, color, theme.meta);
                        body.push_str(&format!("        {line_colored}\n"));
                    }
                }
            }
        }
    }

    let task_label = if filtered {
        "matching tasks"
    } else {
        "pending tasks"
    };
    let summary = if failures > 0 {
        format!("{total} projects attempted, {with_pending} with {task_label}, {failures} failed")
    } else {
        format!("{total} projects checked, {with_pending} with {task_label}")
    };
    let summary_colored = paint(&summary, color, theme.meta);

    if body.is_empty() {
        format!("{summary_colored}\n")
    } else {
        format!("{summary_colored}\n\n{body}")
    }
}

fn pr_state_label(item: &RepoItem) -> Option<&'static str> {
    match item.kind {
        ItemKind::PullRequest => match item.pr_draft {
            Some(true) => Some("draft"),
            Some(false) => Some("ready"),
            None => None,
        },
        ItemKind::Issue => None,
    }
}

fn pr_detail_line(item: &RepoItem) -> Option<String> {
    let mut segs = Vec::new();

    if !item.requested_reviewers.is_empty() || !item.requested_teams.is_empty() {
        let mut requested = item.requested_reviewers.clone();
        requested.extend(
            item.requested_teams
                .iter()
                .map(|team| format!("team:{team}")),
        );
        segs.push(format!("review requested: {}", requested.join(", ")));
    }

    if let Some(extra) = &item.pr_extra
        && let Some(line) = pr_extra_line(extra)
    {
        segs.push(line);
    }

    if segs.is_empty() {
        None
    } else {
        Some(segs.join(" · "))
    }
}

/// The optional third line under a PR: Codex status, unresolved-thread
/// attribution, and human review states. `None` when there is nothing to show.
fn pr_extra_line(extra: &PrExtra) -> Option<String> {
    let mut segs: Vec<String> = Vec::new();

    match extra.codex {
        Some(CodexReaction::Reviewing) => segs.push("codex 👀 reviewing".into()),
        Some(CodexReaction::Lgtm) => segs.push("codex 👍 lgtm".into()),
        None if extra.codex_reviewed => segs.push("codex commented".into()),
        None => {}
    }

    if !extra.unresolved.is_empty() {
        let total: u32 = extra.unresolved.iter().map(|(_, n)| n).sum();
        let authors = extra
            .unresolved
            .iter()
            .map(|(login, _)| login.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        segs.push(format!("{total} unresolved ({authors})"));
    }

    for (login, state) in &extra.reviews {
        segs.push(format!("{login} {}", review_state_label(*state)));
    }

    if let Some(decision) = extra.decision {
        segs.push(review_decision_label(decision).to_owned());
    }

    if segs.is_empty() {
        None
    } else {
        Some(segs.join(" · "))
    }
}

fn review_state_label(state: ReviewState) -> &'static str {
    match state {
        ReviewState::Approved => "approved",
        ReviewState::ChangesRequested => "changes requested",
        ReviewState::Commented => "commented",
        ReviewState::Dismissed => "dismissed",
    }
}

fn review_decision_label(decision: ReviewDecision) -> &'static str {
    match decision {
        ReviewDecision::Approved => "approved",
        ReviewDecision::ChangesRequested => "changes requested",
        ReviewDecision::ReviewRequired => "review required",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::{
        CodexReaction, ItemKind, PrExtra, RepoError, RepoItem, RepoResult, RepoStatus, ReviewState,
    };
    use crate::theme::Theme;

    fn make_item(kind: ItemKind, number: u64, title: &str, days_ago: i64) -> RepoItem {
        let created_at = Utc::now() - chrono::Duration::days(days_ago);
        RepoItem {
            kind,
            number,
            title: title.into(),
            created_at,
            author: "testuser".into(),
            requested_reviewers: vec![],
            requested_teams: vec![],
            pr_draft: None,
            pr_extra: None,
        }
    }

    fn make_pr(number: u64, title: &str, draft: Option<bool>) -> RepoItem {
        RepoItem {
            kind: ItemKind::PullRequest,
            number,
            title: title.into(),
            created_at: Utc::now(),
            author: "testuser".into(),
            requested_reviewers: vec![],
            requested_teams: vec![],
            pr_draft: draft,
            pr_extra: None,
        }
    }

    fn make_pr_with_extra(number: u64, extra: PrExtra) -> RepoItem {
        RepoItem {
            kind: ItemKind::PullRequest,
            number,
            title: "PR title".into(),
            created_at: Utc::now(),
            author: "testuser".into(),
            requested_reviewers: vec![],
            requested_teams: vec![],
            pr_draft: Some(false),
            pr_extra: Some(extra),
        }
    }

    #[test]
    fn no_repos_tracked() {
        let out = render_inner(&[], &Theme::default_theme(), false, 80);
        assert!(out.contains("ghpending add"));
    }

    #[test]
    fn empty_repo_is_skipped_from_listing() {
        let results = vec![RepoResult {
            repo: "owner/empty".into(),
            status: RepoStatus::Items(vec![]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(!out.contains("owner/empty"));
        assert!(!out.contains("nothing pending"));
        assert!(out.contains("1 projects checked, 0 with pending tasks"));
    }

    #[test]
    fn repo_not_found() {
        let results = vec![RepoResult {
            repo: "owner/missing".into(),
            status: RepoStatus::NotFound,
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("owner/missing"));
        assert!(out.contains("not found or no access"));
    }

    #[test]
    fn repo_error_renders_message() {
        let results = vec![RepoResult {
            repo: "owner/flaky".into(),
            status: RepoStatus::Error(RepoError::Api("rate limited".into())),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
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
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("ratatui-org/ratatui"));
        assert!(out.contains("PR"));
        assert!(out.contains("#1842"));
        assert!(out.contains("Fix overflow in Table"));
        assert!(out.contains("ISS"));
        assert!(out.contains("#1840"));
        assert!(out.contains("testuser"));
    }

    #[test]
    fn pull_request_draft_status_is_rendered_subtly() {
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            status: RepoStatus::Items(vec![
                make_pr(3, "Work in progress", Some(true)),
                make_pr(2, "Ready for review", Some(false)),
                make_pr(1, "Unknown state", None),
            ]),
        }];

        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("opened just now ago by testuser · draft"));
        assert!(out.contains("opened just now ago by testuser · ready"));
        assert!(out.contains("opened just now ago by testuser\n"));
    }

    fn extra(codex: Option<CodexReaction>, codex_reviewed: bool) -> PrExtra {
        PrExtra {
            unresolved: vec![],
            codex,
            codex_reviewed,
            reviews: vec![],
            decision: None,
        }
    }

    #[test]
    fn codex_reviewing_and_lgtm_render() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![
                make_pr_with_extra(2, extra(Some(CodexReaction::Reviewing), true)),
                make_pr_with_extra(1, extra(Some(CodexReaction::Lgtm), true)),
            ]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("codex 👀 reviewing"));
        assert!(out.contains("codex 👍 lgtm"));
    }

    #[test]
    fn codex_commented_fallback_when_reviewed_without_reaction() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![make_pr_with_extra(1, extra(None, true))]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("codex commented"));
    }

    #[test]
    fn unresolved_lists_every_author_with_total() {
        let mut e = extra(None, false);
        e.unresolved = vec![
            ("chatgpt-codex-connector".into(), 2),
            ("alvarolopes".into(), 1),
        ];
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![make_pr_with_extra(1, e)]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("3 unresolved (chatgpt-codex-connector, alvarolopes)"));
    }

    #[test]
    fn human_review_states_render() {
        let mut e = extra(None, false);
        e.reviews = vec![
            ("alice".into(), ReviewState::Approved),
            ("bob".into(), ReviewState::ChangesRequested),
        ];
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![make_pr_with_extra(1, e)]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("alice approved"));
        assert!(out.contains("bob changes requested"));
    }

    #[test]
    fn requested_users_and_teams_render_without_graphql_extra() {
        let mut pr = make_pr(1, "Needs review", Some(false));
        pr.requested_reviewers = vec!["alice".into()];
        pr.requested_teams = vec!["owner/backend".into()];
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            status: RepoStatus::Items(vec![pr]),
        }];

        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("review requested: alice, team:owner/backend"));
    }

    #[test]
    fn pr_without_extra_has_no_third_line() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![make_pr(1, "Plain PR", Some(false))]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        // Two lines per item: the item line and the meta line, nothing more.
        let lines: Vec<&str> = out.lines().filter(|l| l.starts_with("        ")).collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn empty_extra_emits_no_third_line() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![make_pr_with_extra(1, extra(None, false))]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        let lines: Vec<&str> = out.lines().filter(|l| l.starts_with("        ")).collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn header_is_just_prefix_and_name() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        let header_line = out.lines().find(|l| l.contains("━━")).unwrap();
        assert_eq!(header_line, "━━ a/b");
    }

    #[test]
    fn two_repos_separated_by_blank_line() {
        let results = vec![
            RepoResult {
                repo: "a/b".into(),
                status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
            },
            RepoResult {
                repo: "c/d".into(),
                status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 2, "y", 0)]),
            },
        ];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        let body = out.split_once("\n\n").unwrap().1;
        assert!(body.contains("\n\n"));
    }

    #[test]
    fn summary_counts_only_repos_with_items() {
        let results = vec![
            RepoResult {
                repo: "a/empty".into(),
                status: RepoStatus::Items(vec![]),
            },
            RepoResult {
                repo: "a/withitems".into(),
                status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
            },
            RepoResult {
                repo: "a/missing".into(),
                status: RepoStatus::NotFound,
            },
            RepoResult {
                repo: "a/flaky".into(),
                status: RepoStatus::Error(RepoError::Api("rate limited".into())),
            },
        ];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("4 projects attempted, 1 with pending tasks, 1 failed"));
        assert!(!out.contains("a/empty"));
        assert!(out.contains("a/withitems"));
        assert!(out.contains("a/missing"));
        assert!(out.contains("a/flaky"));
    }

    #[test]
    fn summary_counts_failures_when_no_pending_tasks() {
        let results = vec![
            RepoResult {
                repo: "a/empty".into(),
                status: RepoStatus::Items(vec![]),
            },
            RepoResult {
                repo: "a/timeout".into(),
                status: RepoStatus::Error(RepoError::Timeout),
            },
        ];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("2 projects attempted, 0 with pending tasks, 1 failed"));
        assert!(out.contains("timeout after 30s"));
    }

    #[test]
    fn all_empty_shows_only_summary() {
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
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert_eq!(out, "2 projects checked, 0 with pending tasks\n");
    }

    #[test]
    fn filtered_summary_describes_matching_tasks() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![]),
        }];
        let out = render_inner_filtered(&results, &Theme::default_theme(), false, 80, true);

        assert_eq!(out, "1 projects checked, 0 with matching tasks\n");
    }

    #[test]
    fn by_name_returns_some_for_known_themes() {
        assert!(Theme::by_name("default").is_some());
        assert!(Theme::by_name("evangelion").is_some());
        assert!(Theme::by_name("nerv").is_some());
    }

    #[test]
    fn by_name_returns_none_for_unknown_theme() {
        assert!(Theme::by_name("matrix").is_none());
        assert!(Theme::by_name("").is_none());
    }

    #[test]
    fn nerv_theme_with_color_produces_ansi_escapes() {
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::nerv(), true, 80);
        assert!(out.contains("\x1b["));
    }

    #[test]
    fn nerv_theme_without_color_has_no_ansi_escapes() {
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::nerv(), false, 80);
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn evangelion_theme_with_color_produces_ansi_escapes() {
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::evangelion(), true, 80);
        assert!(out.contains("\x1b["));
    }
}
