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
}

#[derive(Debug, Error)]
pub enum GithubError {
    #[error("repo not found: {0}")]
    NotFound(String),
    #[error("api error: {0}")]
    Api(#[from] octocrab::Error),
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
    let parts: Vec<&str> = repo.splitn(2, '/').collect();
    if parts.len() != 2 {
        return RepoResult {
            repo: repo.to_owned(),
            status: RepoStatus::NotFound,
        };
    }
    let (owner, name) = (parts[0], parts[1]);

    match fetch_items_inner(crab, owner, name).await {
        Ok(items) => RepoResult {
            repo: repo.to_owned(),
            status: RepoStatus::Items(items),
        },
        Err(GithubError::NotFound(_)) => RepoResult {
            repo: repo.to_owned(),
            status: RepoStatus::NotFound,
        },
        Err(GithubError::Api(e)) => {
            eprintln!("warning: {repo}: {e}");
            RepoResult {
                repo: repo.to_owned(),
                status: RepoStatus::NotFound,
            }
        }
    }
}

async fn fetch_items_inner(
    crab: &Octocrab,
    owner: &str,
    name: &str,
) -> std::result::Result<Vec<RepoItem>, GithubError> {
    let issues_handler = crab.issues(owner, name);
    let issues_future = issues_handler
        .list()
        .state(octocrab::params::State::Open)
        .per_page(50)
        .send();

    let prs_handler = crab.pulls(owner, name);
    let prs_future = prs_handler
        .list()
        .state(octocrab::params::State::Open)
        .per_page(50)
        .send();

    let (issues_res, prs_res) = futures::future::join(issues_future, prs_future).await;

    let issues_page = match issues_res {
        Ok(p) => p,
        Err(octocrab::Error::GitHub { source, .. }) if source.status_code.as_u16() == 404 => {
            return Err(GithubError::NotFound(format!("{owner}/{name}")));
        }
        Err(e) => return Err(GithubError::Api(e)),
    };

    let mut items: Vec<RepoItem> = Vec::new();

    for issue in issues_page.items {
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

    if let Ok(prs_page) = prs_res {
        for pr in prs_page.items {
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
    }

    // Sort: PRs first, then issues; within each group by number descending
    items.sort_by(|a, b| {
        use ItemKind::*;
        match (&a.kind, &b.kind) {
            (PullRequest, Issue) => std::cmp::Ordering::Less,
            (Issue, PullRequest) => std::cmp::Ordering::Greater,
            _ => b.number.cmp(&a.number),
        }
    });

    Ok(items)
}
