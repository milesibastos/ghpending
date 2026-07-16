use std::path::Path;
use std::time::Duration;

use anyhow::{Result, bail};
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use octocrab::Octocrab;
use tokio::time::{self, timeout};

use crate::config::{FilterMode, Filters};
use crate::github::{ItemKind, RepoError, RepoItem, RepoResult, RepoStatus};
use crate::theme::Theme;
use crate::{config, display, github};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONCURRENT_FETCHES: usize = 4;

pub async fn run(
    crab: &Octocrab,
    theme: &Theme,
    cfg_path: &Path,
    repo: Option<&str>,
    cli_authors: &[String],
    cli_review_requested: &[String],
    cli_mode: Option<FilterMode>,
) -> Result<()> {
    let cfg = config::load_from(cfg_path)?;
    let filters = resolve_filters(&cfg.filters, cli_authors, cli_review_requested, cli_mode)?;
    let repos = resolve_repos(&cfg.repos, repo);

    if repos.is_empty() {
        println!("No repos tracked. Run `ghpending add` to get started.");
        return Ok(());
    }

    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .unwrap_or_else(|_| ProgressStyle::default_spinner()),
    );
    spinner.set_message("Fetching…");
    spinner.enable_steady_tick(Duration::from_millis(100));

    let mut results = fetch_repos(crab, &repos).await;
    apply_filters(&mut results, &filters);

    spinner.finish_and_clear();

    let digest = display::render_digest(&results, theme, !filters.is_empty());
    print!("{digest}");

    if all_repo_fetches_failed(&results) {
        anyhow::bail!("all repository fetches failed");
    }

    Ok(())
}

fn resolve_repos(configured: &[String], repo: Option<&str>) -> Vec<String> {
    match repo {
        Some(repo) => vec![repo.to_owned()],
        None => configured.to_vec(),
    }
}

fn resolve_filters(
    saved: &Filters,
    cli_authors: &[String],
    cli_review_requested: &[String],
    cli_mode: Option<FilterMode>,
) -> Result<Filters> {
    let cli_roles_set = !cli_authors.is_empty() || !cli_review_requested.is_empty();
    let mut filters = if cli_roles_set {
        Filters {
            authors: cli_authors.to_vec(),
            review_requested: cli_review_requested.to_vec(),
            mode: cli_mode.unwrap_or(saved.mode),
        }
    } else {
        let mut filters = saved.clone();
        if let Some(mode) = cli_mode {
            filters.mode = mode;
        }
        filters
    };

    normalize_values(&mut filters.authors, "author")?;
    normalize_values(&mut filters.review_requested, "review-requested")?;
    for reviewer in &mut filters.review_requested {
        if reviewer
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("team:"))
        {
            let team = &reviewer[5..];
            let Some((org, slug)) = team.split_once('/') else {
                bail!("invalid team reviewer {reviewer:?}; expected team:ORG/SLUG");
            };
            if org.is_empty() || slug.is_empty() || slug.contains('/') {
                bail!("invalid team reviewer {reviewer:?}; expected team:ORG/SLUG");
            }
            *reviewer = format!("team:{team}");
        }
    }
    Ok(filters)
}

fn normalize_values(values: &mut [String], label: &str) -> Result<()> {
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("{label} filter cannot be empty");
        }
        if trimmed.len() != value.len() {
            *value = trimmed.to_owned();
        }
    }
    Ok(())
}

fn apply_filters(results: &mut [RepoResult], filters: &Filters) {
    if filters.is_empty() {
        return;
    }
    for result in results {
        if let RepoStatus::Items(items) = &mut result.status {
            items.retain(|item| item_matches(item, filters));
        }
    }
}

fn item_matches(item: &RepoItem, filters: &Filters) -> bool {
    let authors_enabled = !filters.authors.is_empty();
    let reviewers_enabled = !filters.review_requested.is_empty();
    let author_matches = authors_enabled
        && filters
            .authors
            .iter()
            .any(|author| author.eq_ignore_ascii_case(&item.author));
    let reviewer_matches = reviewers_enabled
        && item.kind == ItemKind::PullRequest
        && filters.review_requested.iter().any(|reviewer| {
            if let Some(team) = reviewer.strip_prefix("team:") {
                item.requested_teams
                    .iter()
                    .any(|requested| requested.eq_ignore_ascii_case(team))
            } else {
                item.requested_reviewers
                    .iter()
                    .any(|requested| requested.eq_ignore_ascii_case(reviewer))
            }
        });

    match filters.mode {
        FilterMode::Any => author_matches || reviewer_matches,
        FilterMode::All => {
            (!authors_enabled || author_matches) && (!reviewers_enabled || reviewer_matches)
        }
    }
}

async fn fetch_repos(crab: &Octocrab, repos: &[String]) -> Vec<RepoResult> {
    let mut results = vec![None; repos.len()];
    let mut in_flight = FuturesUnordered::new();
    let mut next = 0;

    while next < repos.len() && in_flight.len() < MAX_CONCURRENT_FETCHES {
        let repo = repos[next].clone();
        in_flight.push(fetch_repo_with_timeout(crab, next, repo));
        next += 1;
    }

    let deadline = time::sleep(FETCH_TIMEOUT);
    tokio::pin!(deadline);

    while !in_flight.is_empty() {
        tokio::select! {
            _ = &mut deadline => break,
            Some((index, result)) = in_flight.next() => {
                results[index] = Some(result);

                if next < repos.len() {
                    let repo = repos[next].clone();
                    in_flight.push(fetch_repo_with_timeout(crab, next, repo));
                    next += 1;
                }
            }
        }
    }

    results
        .into_iter()
        .enumerate()
        .map(|(index, result)| result.unwrap_or_else(|| timeout_result(repos[index].clone())))
        .collect()
}

async fn fetch_repo_with_timeout(
    crab: &Octocrab,
    index: usize,
    repo: String,
) -> (usize, RepoResult) {
    let result = match timeout(FETCH_TIMEOUT, github::fetch_repo_items(crab, &repo)).await {
        Ok(result) => result,
        Err(_) => timeout_result(repo),
    };
    (index, result)
}

fn timeout_result(repo: String) -> RepoResult {
    RepoResult {
        repo,
        metadata: None,
        status: RepoStatus::Error(RepoError::Timeout),
    }
}

pub(crate) fn all_repo_fetches_failed(results: &[RepoResult]) -> bool {
    !results.is_empty()
        && results
            .iter()
            .all(|result| matches!(result.status, RepoStatus::Error(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn item(kind: ItemKind, author: &str) -> RepoItem {
        RepoItem {
            kind,
            number: 1,
            title: "item".into(),
            created_at: Utc::now(),
            author: author.into(),
            requested_reviewers: vec![],
            requested_teams: vec![],
            pr_draft: None,
            pr_extra: None,
        }
    }

    fn filters(authors: &[&str], reviewers: &[&str], mode: FilterMode) -> Filters {
        Filters {
            authors: authors.iter().map(|value| (*value).into()).collect(),
            review_requested: reviewers.iter().map(|value| (*value).into()).collect(),
            mode,
        }
    }

    #[test]
    fn author_filter_matches_prs_and_issues_case_insensitively() {
        let filters = filters(&["Alice"], &[], FilterMode::Any);

        assert!(item_matches(
            &item(ItemKind::PullRequest, "alice"),
            &filters
        ));
        assert!(item_matches(&item(ItemKind::Issue, "ALICE"), &filters));
        assert!(!item_matches(&item(ItemKind::Issue, "bob"), &filters));
    }

    #[test]
    fn reviewer_filter_matches_only_current_pr_requests() {
        let filters = filters(&[], &["alice"], FilterMode::Any);
        let mut pr = item(ItemKind::PullRequest, "bob");
        pr.requested_reviewers.push("Alice".into());
        let mut issue = item(ItemKind::Issue, "bob");
        issue.requested_reviewers.push("alice".into());

        assert!(item_matches(&pr, &filters));
        assert!(!item_matches(&issue, &filters));
    }

    #[test]
    fn team_reviewer_requires_explicit_team_selector() {
        let mut pr = item(ItemKind::PullRequest, "bob");
        pr.requested_teams.push("BillinTeam/backend".into());

        assert!(item_matches(
            &pr,
            &filters(&[], &["team:billinteam/BACKEND"], FilterMode::Any)
        ));
        assert!(!item_matches(
            &pr,
            &filters(&[], &["backend"], FilterMode::Any)
        ));
    }

    #[test]
    fn any_mode_matches_either_role() {
        let filters = filters(&["alice"], &["bob"], FilterMode::Any);
        let authored = item(ItemKind::PullRequest, "alice");
        let mut review_requested = item(ItemKind::PullRequest, "carol");
        review_requested.requested_reviewers.push("bob".into());

        assert!(item_matches(&authored, &filters));
        assert!(item_matches(&review_requested, &filters));
    }

    #[test]
    fn all_mode_requires_every_enabled_role() {
        let filters = filters(&["alice"], &["bob"], FilterMode::All);
        let mut both = item(ItemKind::PullRequest, "alice");
        both.requested_reviewers.push("bob".into());

        assert!(item_matches(&both, &filters));
        assert!(!item_matches(
            &item(ItemKind::PullRequest, "alice"),
            &filters
        ));
        assert!(!item_matches(&item(ItemKind::Issue, "alice"), &filters));
    }

    #[test]
    fn cli_roles_replace_saved_roles() {
        let saved = filters(&["saved-author"], &["saved-reviewer"], FilterMode::All);
        let resolved =
            resolve_filters(&saved, &["cli-author".into()], &[], Some(FilterMode::Any)).unwrap();

        assert_eq!(resolved.authors, ["cli-author"]);
        assert!(resolved.review_requested.is_empty());
        assert_eq!(resolved.mode, FilterMode::Any);
    }

    #[test]
    fn cli_roles_keep_saved_mode_without_cli_match() {
        let saved = filters(&["saved-author"], &["saved-reviewer"], FilterMode::All);
        let resolved = resolve_filters(&saved, &["cli-author".into()], &[], None).unwrap();

        assert_eq!(resolved.authors, ["cli-author"]);
        assert!(resolved.review_requested.is_empty());
        assert_eq!(resolved.mode, FilterMode::All);
    }

    #[test]
    fn cli_match_mode_alone_overrides_saved_mode() {
        let saved = filters(&["alice"], &["bob"], FilterMode::Any);
        let resolved = resolve_filters(&saved, &[], &[], Some(FilterMode::All)).unwrap();

        assert_eq!(resolved.authors, saved.authors);
        assert_eq!(resolved.review_requested, saved.review_requested);
        assert_eq!(resolved.mode, FilterMode::All);
    }

    #[test]
    fn invalid_team_selector_is_rejected() {
        let error = resolve_filters(
            &filters(&[], &["team:backend"], FilterMode::Any),
            &[],
            &[],
            None,
        )
        .unwrap_err();

        assert!(error.to_string().contains("expected team:ORG/SLUG"));
    }

    #[test]
    fn one_off_repository_replaces_configured_watch_list() {
        let configured = vec!["owner/one".into(), "owner/two".into()];

        assert_eq!(
            resolve_repos(&configured, Some("other/repo")),
            ["other/repo"]
        );
        assert_eq!(resolve_repos(&configured, None), configured);
    }

    #[test]
    fn filtering_keeps_repository_failures_visible() {
        let mut results = vec![
            RepoResult {
                repo: "a/repo".into(),
                metadata: None,
                status: RepoStatus::Items(vec![item(ItemKind::Issue, "bob")]),
            },
            RepoResult {
                repo: "a/failure".into(),
                metadata: None,
                status: RepoStatus::Error(RepoError::Api("boom".into())),
            },
        ];

        apply_filters(&mut results, &filters(&["alice"], &[], FilterMode::Any));

        assert!(matches!(&results[0].status, RepoStatus::Items(items) if items.is_empty()));
        assert!(matches!(&results[1].status, RepoStatus::Error(_)));
    }

    #[test]
    fn all_repo_fetches_failed_requires_every_result_to_be_error() {
        assert!(all_repo_fetches_failed(&[
            RepoResult {
                repo: "a/b".into(),
                metadata: None,
                status: RepoStatus::Error(RepoError::Timeout),
            },
            RepoResult {
                repo: "c/d".into(),
                metadata: None,
                status: RepoStatus::Error(RepoError::Api("boom".into())),
            },
        ]));

        assert!(!all_repo_fetches_failed(&[RepoResult {
            repo: "a/b".into(),
            metadata: None,
            status: RepoStatus::NotFound,
        }]));

        assert!(!all_repo_fetches_failed(&[RepoResult {
            repo: "a/b".into(),
            metadata: None,
            status: RepoStatus::Items(vec![]),
        }]));

        assert!(!all_repo_fetches_failed(&[]));
    }
}
