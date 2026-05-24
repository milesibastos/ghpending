use std::cmp::Ordering;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use octocrab::Octocrab;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct RepoItem {
    pub kind: ItemKind,
    pub number: u64,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub author: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    PullRequest,
    Issue,
}

#[derive(Debug, Clone)]
pub struct RepoResult {
    pub repo: String,
    pub status: RepoStatus,
}

#[derive(Debug, Clone)]
pub enum RepoStatus {
    Items(Vec<RepoItem>),
    NotFound,
    Error(String),
}

#[derive(Debug, Error)]
pub enum GithubError {
    #[error("repo not found: {0}")]
    NotFound(String),
    #[error("api error: {0}")]
    Api(#[from] octocrab::Error),
}

/// Maps an octocrab result to `GithubError`, treating HTTP 404 as `NotFound`.
fn map_github_err<T>(
    res: std::result::Result<T, octocrab::Error>,
    repo_label: &str,
) -> std::result::Result<T, GithubError> {
    match res {
        Ok(v) => Ok(v),
        Err(octocrab::Error::GitHub { source, .. }) if source.status_code.as_u16() == 404 => {
            Err(GithubError::NotFound(repo_label.to_owned()))
        }
        Err(e) => Err(GithubError::Api(e)),
    }
}

pub fn split_repo(s: &str) -> Option<(&str, &str)> {
    let (owner, name) = s.split_once('/')?;

    if owner.is_empty() || name.is_empty() {
        return None;
    }
    Some((owner, name))
}

pub fn item_cmp(a: &RepoItem, b: &RepoItem) -> Ordering {
    match (&a.kind, &b.kind) {
        (ItemKind::PullRequest, ItemKind::Issue) => Ordering::Less,
        (ItemKind::Issue, ItemKind::PullRequest) => Ordering::Greater,
        _ => b.number.cmp(&a.number),
    }
}

pub async fn list_user_repos(crab: &Octocrab, username: &str) -> Result<Vec<String>> {
    let first_page = crab
        .users(username)
        .repos()
        .r#type(octocrab::params::users::repos::Type::Owner)
        .per_page(100)
        .send()
        .await
        .context("listing user repositories")?;

    let all_pages = crab
        .all_pages(first_page)
        .await
        .context("paginating user repositories")?;

    let mut names: Vec<String> = all_pages.into_iter().filter_map(|r| r.full_name).collect();
    names.sort();
    Ok(names)
}

pub async fn fetch_repo_items(crab: &Octocrab, repo: &str) -> RepoResult {
    let Some((owner, name)) = split_repo(repo) else {
        return RepoResult {
            repo: repo.to_owned(),
            status: RepoStatus::NotFound,
        };
    };

    match fetch_items_inner(crab, owner, name).await {
        Ok(items) => RepoResult {
            repo: repo.to_owned(),
            status: RepoStatus::Items(items),
        },
        Err(GithubError::NotFound(_)) => RepoResult {
            repo: repo.to_owned(),
            status: RepoStatus::NotFound,
        },
        Err(GithubError::Api(e)) => RepoResult {
            repo: repo.to_owned(),
            status: RepoStatus::Error(e.to_string()),
        },
    }
}

async fn fetch_items_inner(
    crab: &Octocrab,
    owner: &str,
    name: &str,
) -> std::result::Result<Vec<RepoItem>, GithubError> {
    let label = format!("{owner}/{name}");

    let issues_handler = crab.issues(owner, name);
    let issues_future = issues_handler
        .list()
        .state(octocrab::params::State::Open)
        .per_page(100)
        .send();

    let prs_handler = crab.pulls(owner, name);
    let prs_future = prs_handler
        .list()
        .state(octocrab::params::State::Open)
        .per_page(100)
        .send();

    let (issues_res, prs_res) = futures::future::join(issues_future, prs_future).await;

    let issues_page = map_github_err(issues_res, &label)?;
    let prs_page = map_github_err(prs_res, &label)?;

    let all_issues = crab
        .all_pages(issues_page)
        .await
        .map_err(GithubError::Api)?;
    let all_prs = crab.all_pages(prs_page).await.map_err(GithubError::Api)?;

    let mut items: Vec<RepoItem> = Vec::new();

    for issue in all_issues {
        // Skip PRs that appear in the issues endpoint
        if issue.pull_request.is_some() {
            continue;
        }
        let author = issue.user.login.clone();
        let created_at = issue.created_at;
        items.push(RepoItem {
            kind: ItemKind::Issue,
            number: issue.number,
            title: issue.title,
            created_at,
            author,
        });
    }

    for pr in all_prs {
        let author = pr.user.login.clone();
        let created_at = pr.created_at;
        items.push(RepoItem {
            kind: ItemKind::PullRequest,
            number: pr.number,
            title: pr.title,
            created_at,
            author,
        });
    }

    // Sort: PRs first, then issues; within each group by number descending
    items.sort_by(item_cmp);

    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(kind: ItemKind, number: u64) -> RepoItem {
        RepoItem {
            kind,
            number,
            title: format!("item {number}"),
            created_at: Utc::now(),
            author: "user".into(),
        }
    }

    #[test]
    fn split_repo_valid() {
        assert_eq!(split_repo("a/b"), Some(("a", "b")));
    }

    #[test]
    fn split_repo_no_slash() {
        assert_eq!(split_repo("abc"), None);
    }

    #[test]
    fn split_repo_trailing_slash() {
        assert_eq!(split_repo("a/"), None);
    }

    #[test]
    fn split_repo_leading_slash() {
        assert_eq!(split_repo("/b"), None);
    }

    #[test]
    fn split_repo_many_slashes() {
        // splitn(2) gives ("a", "b/c") — name contains a slash, which is fine
        assert_eq!(split_repo("a/b/c"), Some(("a", "b/c")));
    }

    #[test]
    fn item_cmp_sorts_prs_before_issues_then_number_desc() {
        let mut items = [
            make_item(ItemKind::Issue, 5),
            make_item(ItemKind::PullRequest, 2),
            make_item(ItemKind::Issue, 10),
            make_item(ItemKind::PullRequest, 8),
        ];
        items.sort_by(item_cmp);
        let numbers: Vec<u64> = items.iter().map(|i| i.number).collect();
        assert_eq!(numbers, vec![8, 2, 10, 5]);
    }
}
