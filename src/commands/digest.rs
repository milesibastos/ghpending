use std::time::Duration;

use anyhow::Result;
use futures::stream::{FuturesUnordered, StreamExt};
use indicatif::{ProgressBar, ProgressStyle};
use octocrab::Octocrab;
use tokio::time::{self, timeout};

use crate::github::{RepoError, RepoResult, RepoStatus};
use crate::{config, display, github};

const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_CONCURRENT_FETCHES: usize = 4;

pub async fn run(crab: &Octocrab) -> Result<()> {
    let cfg = config::load()?;

    if cfg.repos.is_empty() {
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

    let results = fetch_repos(crab, &cfg.repos).await;

    spinner.finish_and_clear();

    let digest = display::render_digest(&results);
    print!("{digest}");

    if all_repo_fetches_failed(&results) {
        anyhow::bail!("all repository fetches failed");
    }

    Ok(())
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

    #[test]
    fn all_repo_fetches_failed_requires_every_result_to_be_error() {
        assert!(all_repo_fetches_failed(&[
            RepoResult {
                repo: "a/b".into(),
                status: RepoStatus::Error(RepoError::Timeout),
            },
            RepoResult {
                repo: "c/d".into(),
                status: RepoStatus::Error(RepoError::Api("boom".into())),
            },
        ]));

        assert!(!all_repo_fetches_failed(&[RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::NotFound,
        }]));

        assert!(!all_repo_fetches_failed(&[RepoResult {
            repo: "a/b".into(),
            status: RepoStatus::Items(vec![]),
        }]));

        assert!(!all_repo_fetches_failed(&[]));
    }
}
