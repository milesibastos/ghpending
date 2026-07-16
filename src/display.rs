use chrono::{DateTime, Utc};
use owo_colors::Style;
use terminal_size::{Width, terminal_size};

use crate::format::{relative_time, truncate_title};
use crate::github::{
    CheckState, CheckSummary, CodexReaction, ItemKind, MergeReadiness, PrExtra, RepoItem,
    RepoMetadata, RepoResult, RepoStatus, ReviewDecision, ReviewState, is_codex_actor,
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
        let mut metadata_segments = repo_metadata_segments(result.metadata.as_ref(), &now);
        while let Some(metadata) =
            (!metadata_segments.is_empty()).then(|| metadata_segments.join(" · "))
        {
            let header_width = 3 + result.repo.chars().count() + 3 + metadata.chars().count();
            if header_width <= width {
                break;
            }
            metadata_segments.pop();
        }
        body.push_str(&format!("━━ {repo_colored}"));
        if !metadata_segments.is_empty() {
            let metadata = paint(
                &format!(" · {}", metadata_segments.join(" · ")),
                color,
                theme.meta,
            );
            body.push_str(&metadata);
        }
        body.push('\n');

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
                    let meta_colored = styled_pr_meta_line(&meta, item, theme, color);
                    body.push_str(&format!("        {meta_colored}\n"));

                    if let Some(line) = styled_pr_detail_line(item, theme, color) {
                        body.push_str(&format!("        {line}\n"));
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
    let project_label = if total == 1 { "project" } else { "projects" };
    let summary = if failures > 0 {
        format!(
            "{total} {project_label} attempted, {with_pending} with {task_label}, {failures} failed"
        )
    } else {
        format!("{total} {project_label} checked, {with_pending} with {task_label}")
    };
    let summary_colored = paint(&summary, color, theme.meta);

    if body.is_empty() {
        format!("{summary_colored}\n")
    } else {
        format!("{summary_colored}\n\n{body}")
    }
}

fn repo_metadata_segments(metadata: Option<&RepoMetadata>, now: &DateTime<Utc>) -> Vec<String> {
    let Some(metadata) = metadata else {
        return Vec::new();
    };
    let mut segments = Vec::new();

    if let Some(release) = &metadata.release {
        let kind = if release.is_prerelease {
            "prerelease"
        } else {
            "release"
        };
        segments.push(format!(
            "{kind} {} ({})",
            release.tag_name,
            relative_time(&release.published_at, now)
        ));
    }
    if let Some(tag) = &metadata.recent_tag
        && metadata
            .release
            .as_ref()
            .is_none_or(|release| release.tag_name != *tag)
    {
        segments.push(format!("tag {tag}"));
    }

    segments
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

#[derive(Clone, Copy)]
enum DetailTone {
    Meta,
    Success,
    Warning,
    Error,
}

struct DetailSegment {
    text: String,
    tone: DetailTone,
}

impl DetailSegment {
    fn meta(text: String) -> Self {
        DetailSegment {
            text,
            tone: DetailTone::Meta,
        }
    }
}

fn pr_detail_segments(item: &RepoItem) -> Vec<DetailSegment> {
    let mut segs = Vec::new();
    let mut awaiting_review = Vec::new();
    let mut awaiting_rereview = Vec::new();

    for reviewer in &item.requested_reviewers {
        if item.pr_extra.as_ref().is_some_and(|extra| {
            extra.prior_reviewers.iter().any(|prior| {
                prior.eq_ignore_ascii_case(reviewer)
                    || (is_codex_actor(prior) && is_codex_actor(reviewer))
            })
        }) {
            awaiting_rereview.push(reviewer.clone());
        } else {
            awaiting_review.push(reviewer.clone());
        }
    }
    awaiting_review.extend(
        item.requested_teams
            .iter()
            .map(|team| format!("team:{team}")),
    );
    let has_review_requests = !awaiting_review.is_empty() || !awaiting_rereview.is_empty();

    if let Some(extra) = &item.pr_extra
        && let Some(line) = pr_extra_line(extra, has_review_requests, &awaiting_rereview)
    {
        segs.push(DetailSegment::meta(line));
    }

    if !awaiting_rereview.is_empty() {
        segs.push(DetailSegment::meta(format!(
            "awaiting re-review ({}): {}",
            awaiting_rereview.len(),
            awaiting_rereview.join(", ")
        )));
    }
    if !awaiting_review.is_empty() {
        segs.push(DetailSegment::meta(format!(
            "awaiting review ({}): {}",
            awaiting_review.len(),
            awaiting_review.join(", ")
        )));
    }

    segs
}

fn pr_status_segments(item: &RepoItem) -> Vec<DetailSegment> {
    let mut segs = Vec::new();
    if let Some(extra) = &item.pr_extra {
        if let Some(readiness) = extra.merge_readiness {
            segs.push(merge_readiness_segment(readiness));
        }
        if let Some(checks) = &extra.checks {
            segs.push(check_segment(checks));
        }
    }
    segs
}

#[cfg(test)]
fn pr_detail_line(item: &RepoItem) -> Option<String> {
    plain_segments(pr_detail_segments(item))
}

#[cfg(test)]
fn pr_status_line(item: &RepoItem) -> Option<String> {
    plain_segments(pr_status_segments(item))
}

#[cfg(test)]
fn plain_segments(segs: Vec<DetailSegment>) -> Option<String> {
    if segs.is_empty() {
        None
    } else {
        Some(
            segs.into_iter()
                .map(|segment| segment.text)
                .collect::<Vec<_>>()
                .join(" · "),
        )
    }
}

fn styled_pr_meta_line(meta: &str, item: &RepoItem, theme: &Theme, color: bool) -> String {
    let mut segs = vec![DetailSegment::meta(meta.to_owned())];
    segs.extend(pr_status_segments(item));
    styled_segments(segs, theme, color).expect("metadata always supplies one segment")
}

fn styled_pr_detail_line(item: &RepoItem, theme: &Theme, color: bool) -> Option<String> {
    styled_segments(pr_detail_segments(item), theme, color)
}

fn styled_segments(segs: Vec<DetailSegment>, theme: &Theme, color: bool) -> Option<String> {
    if segs.is_empty() {
        return None;
    }
    let separator = paint(" · ", color, theme.meta);
    Some(
        segs.into_iter()
            .map(|segment| {
                let style = match segment.tone {
                    DetailTone::Meta => theme.meta,
                    DetailTone::Success => theme.success,
                    DetailTone::Warning => theme.warning,
                    DetailTone::Error => theme.error,
                };
                paint(&segment.text, color, style)
            })
            .collect::<Vec<_>>()
            .join(&separator),
    )
}

fn check_segment(checks: &CheckSummary) -> DetailSegment {
    let (label, tone) = match checks.state {
        CheckState::Passed => ("checks passed", DetailTone::Success),
        CheckState::Pending => ("checks pending", DetailTone::Warning),
        CheckState::Failed => ("checks failed", DetailTone::Error),
    };
    let text = if checks.state == CheckState::Passed {
        format!("{label} ({})", checks.total)
    } else if checks.names.is_empty() {
        label.to_owned()
    } else {
        format!(
            "{label} ({}): {}",
            checks.names.len(),
            checks.names.join(", ")
        )
    };
    DetailSegment { text, tone }
}

fn merge_readiness_segment(readiness: MergeReadiness) -> DetailSegment {
    let (text, tone) = match readiness {
        MergeReadiness::Ready => ("merge ready", DetailTone::Success),
        MergeReadiness::Blocked => ("merge blocked", DetailTone::Error),
        MergeReadiness::Behind => ("merge behind", DetailTone::Warning),
        MergeReadiness::Unstable => ("merge unstable", DetailTone::Warning),
        MergeReadiness::Conflicts => ("merge conflicts", DetailTone::Error),
        MergeReadiness::Hooks => ("merge hooks", DetailTone::Warning),
        MergeReadiness::Unknown => ("merge unknown", DetailTone::Warning),
    };
    DetailSegment {
        text: text.to_owned(),
        tone,
    }
}

/// The optional third line under a PR: Codex status, unresolved-thread
/// attribution, and human review states. `None` when there is nothing to show.
fn pr_extra_line(
    extra: &PrExtra,
    has_review_requests: bool,
    awaiting_rereview: &[String],
) -> Option<String> {
    let mut segs: Vec<String> = Vec::new();

    if !awaiting_rereview.iter().any(|login| is_codex_actor(login)) {
        match extra.codex {
            Some(CodexReaction::Reviewing) => segs.push("codex 👀 reviewing".into()),
            Some(CodexReaction::Lgtm) => segs.push("codex 👍 lgtm".into()),
            None if extra.codex_reviewed
                && !extra
                    .unresolved
                    .iter()
                    .any(|(author, _)| is_codex_actor(author)) =>
            {
                segs.push("codex commented".into());
            }
            None => {}
        }
    }

    if !extra.unresolved.is_empty() {
        let total: u32 = extra.unresolved.iter().map(|(_, n)| n).sum();
        let authors = extra
            .unresolved
            .iter()
            .map(|(login, _)| login.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        segs.push(format!("{total} unresolved by {authors}"));
    }

    let mut review_groups: Vec<(ReviewState, Vec<&str>)> = Vec::new();
    for (login, state) in &extra.reviews {
        if awaiting_rereview
            .iter()
            .any(|reviewer| reviewer.eq_ignore_ascii_case(login))
        {
            continue;
        }
        if *state == ReviewState::Commented
            && extra
                .unresolved
                .iter()
                .any(|(author, _)| author.eq_ignore_ascii_case(login))
        {
            continue;
        }
        if let Some((_, logins)) = review_groups
            .iter_mut()
            .find(|(group_state, _)| group_state == state)
        {
            logins.push(login);
        } else {
            review_groups.push((*state, vec![login]));
        }
    }
    review_groups.sort_by_key(|(state, _)| match state {
        ReviewState::Approved => 0,
        ReviewState::ChangesRequested => 1,
        ReviewState::Commented => 2,
        ReviewState::Dismissed => 3,
    });
    for (state, logins) in &review_groups {
        segs.push(format!(
            "{} ({}): {}",
            review_state_label(*state),
            logins.len(),
            logins.join(", ")
        ));
    }

    if let Some(decision) = extra.decision
        && !(decision == ReviewDecision::ReviewRequired && has_review_requests)
        && !review_groups
            .iter()
            .any(|(state, _)| review_state_label(*state) == review_decision_label(decision))
    {
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
        CheckState, CheckSummary, CodexReaction, ItemKind, MergeReadiness, PrExtra,
        ReleaseMetadata, RepoError, RepoItem, RepoResult, RepoStatus, ReviewState,
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
            metadata: None,
            status: RepoStatus::Items(vec![]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(!out.contains("owner/empty"));
        assert!(!out.contains("nothing pending"));
        assert!(out.contains("1 project checked, 0 with pending tasks"));
    }

    #[test]
    fn repo_not_found() {
        let results = vec![RepoResult {
            repo: "owner/missing".into(),
            metadata: None,
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
            metadata: None,
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
            metadata: None,
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
            metadata: None,
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
            prior_reviewers: vec![],
            decision: None,
            checks: None,
            merge_readiness: None,
        }
    }

    #[test]
    fn codex_reviewing_and_lgtm_render() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            metadata: None,
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
            metadata: None,
            status: RepoStatus::Items(vec![make_pr_with_extra(1, extra(None, true))]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("codex commented"));
    }

    #[test]
    fn codex_commented_is_hidden_when_codex_has_an_unresolved_thread() {
        let mut e = extra(None, true);
        e.unresolved = vec![("chatgpt-codex-connector".into(), 1)];

        assert_eq!(
            pr_extra_line(&e, false, &[]).as_deref(),
            Some("1 unresolved by chatgpt-codex-connector")
        );
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
            metadata: None,
            status: RepoStatus::Items(vec![make_pr_with_extra(1, e)]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("3 unresolved by chatgpt-codex-connector, alvarolopes"));
    }

    #[test]
    fn human_review_states_are_grouped_and_redundant_decision_is_omitted() {
        let mut e = extra(None, false);
        e.reviews = vec![
            ("bob".into(), ReviewState::ChangesRequested),
            ("alice".into(), ReviewState::Approved),
            ("carol".into(), ReviewState::Approved),
            ("dave".into(), ReviewState::Commented),
        ];
        e.decision = Some(ReviewDecision::Approved);

        assert_eq!(
            pr_extra_line(&e, false, &[]).as_deref(),
            Some("approved (2): alice, carol · changes requested (1): bob · commented (1): dave")
        );
    }

    #[test]
    fn distinct_review_decision_remains_after_grouped_reviews() {
        let mut e = extra(None, false);
        e.reviews = vec![("anbillin".into(), ReviewState::Commented)];
        e.decision = Some(ReviewDecision::ReviewRequired);

        assert_eq!(
            pr_extra_line(&e, false, &[]).as_deref(),
            Some("commented (1): anbillin · review required")
        );
    }

    #[test]
    fn requested_and_approved_reviewers_render_as_groups() {
        let mut e = extra(None, false);
        e.reviews = vec![
            ("anbillin".into(), ReviewState::Approved),
            ("JorgeBillin".into(), ReviewState::Approved),
        ];
        e.decision = Some(ReviewDecision::Approved);
        let mut pr = make_pr_with_extra(1, e);
        pr.requested_reviewers = vec!["milesibastos".into(), "mishamaliga".into()];

        assert_eq!(
            pr_detail_line(&pr).as_deref(),
            Some(
                "approved (2): anbillin, JorgeBillin · awaiting review (2): milesibastos, mishamaliga"
            )
        );
    }

    #[test]
    fn prior_reviewer_with_active_request_is_explicitly_awaiting_rereview() {
        let mut e = extra(None, false);
        e.reviews = vec![("anbillin".into(), ReviewState::Commented)];
        e.prior_reviewers = vec!["AnBillin".into()];
        let mut pr = make_pr_with_extra(1, e);
        pr.requested_reviewers = vec![
            "mdo2".into(),
            "mishamaliga".into(),
            "JorgeBillin".into(),
            "sergiopanaderobillin".into(),
            "anbillin".into(),
        ];

        assert_eq!(
            pr_detail_line(&pr).as_deref(),
            Some(
                "awaiting re-review (1): anbillin · awaiting review (4): mdo2, mishamaliga, JorgeBillin, sergiopanaderobillin"
            )
        );
    }

    #[test]
    fn codex_login_variants_are_matched_for_rereview() {
        let mut e = extra(Some(CodexReaction::Lgtm), true);
        e.prior_reviewers = vec!["chatgpt-codex-connector".into()];
        let mut pr = make_pr_with_extra(1, e);
        pr.requested_reviewers = vec!["chatgpt-codex-connector[bot]".into()];

        assert_eq!(
            pr_detail_line(&pr).as_deref(),
            Some("awaiting re-review (1): chatgpt-codex-connector[bot]")
        );
    }

    #[test]
    fn unresolved_comment_and_requested_review_collapse_to_actionable_signals() {
        let mut e = extra(None, false);
        e.unresolved = vec![("anbillin".into(), 1)];
        e.reviews = vec![("anbillin".into(), ReviewState::Commented)];
        e.decision = Some(ReviewDecision::ReviewRequired);
        let mut pr = make_pr_with_extra(1, e);
        pr.requested_reviewers = vec![
            "mdo2".into(),
            "mishamaliga".into(),
            "JorgeBillin".into(),
            "sergiopanaderobillin".into(),
        ];

        assert_eq!(
            pr_detail_line(&pr).as_deref(),
            Some(
                "1 unresolved by anbillin · awaiting review (4): mdo2, mishamaliga, JorgeBillin, sergiopanaderobillin"
            )
        );
    }

    #[test]
    fn comment_from_another_reviewer_remains_with_unresolved_threads() {
        let mut e = extra(None, false);
        e.unresolved = vec![("alice".into(), 1)];
        e.reviews = vec![("bob".into(), ReviewState::Commented)];

        assert_eq!(
            pr_extra_line(&e, false, &[]).as_deref(),
            Some("1 unresolved by alice · commented (1): bob")
        );
    }

    #[test]
    fn requested_users_and_teams_render_without_graphql_extra() {
        let mut pr = make_pr(1, "Needs review", Some(false));
        pr.requested_reviewers = vec!["alice".into()];
        pr.requested_teams = vec!["owner/backend".into()];
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            metadata: None,
            status: RepoStatus::Items(vec![pr]),
        }];

        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        assert!(out.contains("awaiting review (2): alice, team:owner/backend"));
    }

    #[test]
    fn pr_without_extra_has_no_third_line() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            metadata: None,
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
            metadata: None,
            status: RepoStatus::Items(vec![make_pr_with_extra(1, extra(None, false))]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        let lines: Vec<&str> = out.lines().filter(|l| l.starts_with("        ")).collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn checks_and_merge_render_on_metadata_before_review_detail() {
        let mut e = extra(None, false);
        e.checks = Some(CheckSummary {
            state: CheckState::Failed,
            total: 4,
            names: vec!["cargo-test".into(), "clippy".into()],
        });
        e.merge_readiness = Some(MergeReadiness::Blocked);
        e.reviews = vec![("alice".into(), ReviewState::Approved)];

        let pr = make_pr_with_extra(1, e);
        assert_eq!(
            pr_status_line(&pr).as_deref(),
            Some("merge blocked · checks failed (2): cargo-test, clippy")
        );
        assert_eq!(pr_detail_line(&pr).as_deref(), Some("approved (1): alice"));
    }

    #[test]
    fn pending_and_passed_checks_have_compact_counts() {
        let mut pending = extra(None, false);
        pending.checks = Some(CheckSummary {
            state: CheckState::Pending,
            total: 3,
            names: vec!["cargo-test".into(), "clippy".into()],
        });
        let mut passed = extra(None, false);
        passed.checks = Some(CheckSummary {
            state: CheckState::Passed,
            total: 4,
            names: vec![],
        });

        assert_eq!(
            pr_status_line(&make_pr_with_extra(2, pending)).as_deref(),
            Some("checks pending (2): cargo-test, clippy")
        );
        assert_eq!(
            pr_status_line(&make_pr_with_extra(1, passed)).as_deref(),
            Some("checks passed (4)")
        );
    }

    #[test]
    fn check_and_merge_segments_use_semantic_colors() {
        let mut e = extra(None, false);
        e.checks = Some(CheckSummary {
            state: CheckState::Failed,
            total: 2,
            names: vec!["cargo-test".into(), "clippy".into()],
        });
        e.merge_readiness = Some(MergeReadiness::Behind);
        let theme = Theme::default_theme();

        let line = styled_pr_meta_line(
            "opened just now ago by testuser · ready",
            &make_pr_with_extra(1, e),
            &theme,
            true,
        );

        assert!(line.contains(&paint(
            "checks failed (2): cargo-test, clippy",
            true,
            theme.error
        )));
        assert!(line.contains(&paint("merge behind", true, theme.warning)));
    }

    #[test]
    fn check_and_merge_segments_respect_no_color() {
        let mut e = extra(None, false);
        e.checks = Some(CheckSummary {
            state: CheckState::Failed,
            total: 2,
            names: vec!["cargo-test".into(), "clippy".into()],
        });
        e.merge_readiness = Some(MergeReadiness::Conflicts);
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            metadata: None,
            status: RepoStatus::Items(vec![make_pr_with_extra(1, e)]),
        }];

        let out = render_inner(&results, &Theme::default_theme(), false, 80);

        assert!(!out.contains("\x1b["));
        let lines = out
            .lines()
            .filter(|line| line.starts_with("        "))
            .collect::<Vec<_>>();
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("ready · merge conflicts · checks failed (2): cargo-test, clippy")
        );
    }

    #[test]
    fn header_is_just_prefix_and_name() {
        let results = vec![RepoResult {
            repo: "a/b".into(),
            metadata: None,
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::default_theme(), false, 80);
        let header_line = out.lines().find(|l| l.contains("━━")).unwrap();
        assert_eq!(header_line, "━━ a/b");
    }

    #[test]
    fn repository_metadata_covers_release_and_tag_states() {
        let now = DateTime::parse_from_rfc3339("2026-07-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let release = ReleaseMetadata {
            tag_name: "v2.4.0".into(),
            published_at: DateTime::parse_from_rfc3339("2026-07-10T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
            is_prerelease: false,
        };

        assert_eq!(
            repo_metadata_segments(
                Some(&RepoMetadata {
                    release: Some(release.clone()),
                    recent_tag: Some("v2.4.0".into()),
                }),
                &now,
            ),
            ["release v2.4.0 (6d)"]
        );
        assert_eq!(
            repo_metadata_segments(
                Some(&RepoMetadata {
                    release: Some(ReleaseMetadata {
                        is_prerelease: true,
                        ..release
                    }),
                    recent_tag: Some("v2.5.0-rc.1".into()),
                }),
                &now,
            ),
            ["prerelease v2.4.0 (6d)", "tag v2.5.0-rc.1"]
        );
        assert_eq!(
            repo_metadata_segments(
                Some(&RepoMetadata {
                    release: None,
                    recent_tag: Some("v1.8.0".into()),
                }),
                &now,
            ),
            ["tag v1.8.0"]
        );
        assert!(repo_metadata_segments(Some(&RepoMetadata::default()), &now).is_empty());
    }

    #[test]
    fn repository_header_drops_secondary_tag_when_narrow() {
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            metadata: Some(RepoMetadata {
                release: Some(ReleaseMetadata {
                    tag_name: "v2.4.0".into(),
                    published_at: Utc::now() - chrono::Duration::days(6),
                    is_prerelease: false,
                }),
                recent_tag: Some("v2.5.0-rc.1".into()),
            }),
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];

        let out = render_inner(&results, &Theme::default_theme(), false, 40);
        let header_line = out.lines().find(|line| line.contains("━━")).unwrap();

        assert_eq!(header_line, "━━ owner/repo · release v2.4.0 (6d)");
    }

    #[test]
    fn two_repos_separated_by_blank_line() {
        let results = vec![
            RepoResult {
                repo: "a/b".into(),
                metadata: None,
                status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
            },
            RepoResult {
                repo: "c/d".into(),
                metadata: None,
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
                metadata: None,
                status: RepoStatus::Items(vec![]),
            },
            RepoResult {
                repo: "a/withitems".into(),
                metadata: None,
                status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
            },
            RepoResult {
                repo: "a/missing".into(),
                metadata: None,
                status: RepoStatus::NotFound,
            },
            RepoResult {
                repo: "a/flaky".into(),
                metadata: None,
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
                metadata: None,
                status: RepoStatus::Items(vec![]),
            },
            RepoResult {
                repo: "a/timeout".into(),
                metadata: None,
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
                metadata: None,
                status: RepoStatus::Items(vec![]),
            },
            RepoResult {
                repo: "c/d".into(),
                metadata: None,
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
            metadata: None,
            status: RepoStatus::Items(vec![]),
        }];
        let out = render_inner_filtered(&results, &Theme::default_theme(), false, 80, true);

        assert_eq!(out, "1 project checked, 0 with matching tasks\n");
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
            metadata: None,
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::nerv(), true, 80);
        assert!(out.contains("\x1b["));
    }

    #[test]
    fn nerv_theme_without_color_has_no_ansi_escapes() {
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            metadata: None,
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::nerv(), false, 80);
        assert!(!out.contains("\x1b["));
    }

    #[test]
    fn evangelion_theme_with_color_produces_ansi_escapes() {
        let results = vec![RepoResult {
            repo: "owner/repo".into(),
            metadata: None,
            status: RepoStatus::Items(vec![make_item(ItemKind::Issue, 1, "x", 0)]),
        }];
        let out = render_inner(&results, &Theme::evangelion(), true, 80);
        assert!(out.contains("\x1b["));
    }
}
