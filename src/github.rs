use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use octocrab::Octocrab;
use serde::{Deserialize, Deserializer};
use thiserror::Error;

/// Enrichment is bounded on its own, well under the digest's 30s per-repo
/// deadline, so a slow GraphQL endpoint can never consume the whole budget and
/// discard the REST items that already resolved.
const ENRICH_TIMEOUT: Duration = Duration::from_secs(8);

/// The login of the OpenAI Codex review bot. It surfaces as
/// `chatgpt-codex-connector[bot]` on reactions but `chatgpt-codex-connector`
/// on reviews, so match with [`is_codex_actor`], never `==`.
const CODEX_LOGIN: &str = "chatgpt-codex-connector";

#[derive(Debug, Clone)]
pub struct RepoItem {
    pub kind: ItemKind,
    pub number: u64,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub author: String,
    /// Users currently requested to review this PR. Empty for issues.
    pub requested_reviewers: Vec<String>,
    /// Requested teams as `ORG/SLUG`. Empty for issues.
    pub requested_teams: Vec<String>,
    pub pr_draft: Option<bool>,
    /// Best-effort GraphQL enrichment; `None` for issues and for PRs whose
    /// enrichment query failed or did not resolve them.
    pub pr_extra: Option<PrExtra>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemKind {
    PullRequest,
    Issue,
}

/// Extra PR-only signals fetched via GraphQL. Assembled from the pure helpers
/// below so the wire shape stays at the edge.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrExtra {
    /// Unresolved review threads attributed to their opening author, most
    /// first. `(login, count)`.
    pub unresolved: Vec<(String, u32)>,
    /// Codex's current PR-body reaction, its live review status.
    pub codex: Option<CodexReaction>,
    /// Whether Codex authored any review — the "codex commented" fallback when
    /// it has left no reaction.
    pub codex_reviewed: bool,
    /// Latest review state per non-Codex reviewer.
    pub reviews: Vec<(String, ReviewState)>,
    /// Logins found in recent submitted-review history, used to distinguish an
    /// active first-time request from a re-review request.
    pub prior_reviewers: Vec<String>,
    /// GitHub's rollup, populated only when an opinionated review or branch
    /// protection forces it (usually `None` here).
    pub decision: Option<ReviewDecision>,
    /// Status checks and legacy commit statuses on the PR's head commit.
    pub checks: Option<CheckSummary>,
    /// GitHub's base-aware merge state for the PR.
    pub merge_readiness: Option<MergeReadiness>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckSummary {
    pub state: CheckState,
    pub total: u32,
    /// Check names matching the aggregate failure or pending state.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckState {
    Passed,
    Pending,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeReadiness {
    Ready,
    Blocked,
    Behind,
    Unstable,
    Conflicts,
    Hooks,
    Unknown,
}

/// Codex's PR-body reaction: 👀 while reviewing, 👍 once satisfied. Codex never
/// submits an APPROVED review, so this is its only "lgtm" signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexReaction {
    Reviewing,
    Lgtm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// Matches the Codex bot regardless of the `[bot]` suffix GitHub appends on
/// reaction users but not review authors.
pub fn is_codex_actor(login: &str) -> bool {
    login.trim_end_matches("[bot]") == CODEX_LOGIN
}

/// Aggregates unresolved review threads by their opening comment's author,
/// sorted by count descending then login for stable output.
fn unresolved_by_author(threads: &[ThreadInfo]) -> Vec<(String, u32)> {
    let mut counts: HashMap<&str, u32> = HashMap::new();
    for thread in threads {
        if thread.resolved {
            continue;
        }
        if let Some(author) = &thread.opener {
            *counts.entry(author.as_str()).or_insert(0) += 1;
        }
    }
    let mut out: Vec<(String, u32)> = counts
        .into_iter()
        .map(|(login, count)| (login.to_owned(), count))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Picks Codex's reaction, preferring 👍 (done) over 👀 (still reviewing) when
/// both are somehow present.
fn codex_reaction(reactions: &[ReactionInfo]) -> Option<CodexReaction> {
    let mut found: Option<CodexReaction> = None;
    for reaction in reactions {
        if !is_codex_actor(&reaction.login) {
            continue;
        }
        match reaction.content.as_str() {
            "THUMBS_UP" => return Some(CodexReaction::Lgtm),
            "EYES" => found = Some(CodexReaction::Reviewing),
            _ => {}
        }
    }
    found
}

/// Splits latest reviews into per-human-reviewer states (Codex excluded, since
/// its status rides the reaction) and a flag for whether Codex reviewed at all.
fn collapse_reviews(reviews: &[ReviewInfo]) -> (Vec<(String, ReviewState)>, bool) {
    let mut humans: Vec<(String, ReviewState)> = Vec::new();
    let mut codex_reviewed = false;
    for review in reviews {
        if is_codex_actor(&review.login) {
            codex_reviewed = true;
            continue;
        }
        if let Some(state) = review_state_from(&review.state) {
            humans.push((review.login.clone(), state));
        }
    }
    (humans, codex_reviewed)
}

fn prior_reviewers(reviews: &[ReviewInfo]) -> Vec<String> {
    let mut logins = Vec::new();
    for review in reviews {
        if review_state_from(&review.state).is_none() {
            continue;
        }
        if !logins
            .iter()
            .any(|login: &String| login.eq_ignore_ascii_case(&review.login))
        {
            logins.push(review.login.clone());
        }
    }
    logins
}

fn review_state_from(state: &str) -> Option<ReviewState> {
    match state {
        "APPROVED" => Some(ReviewState::Approved),
        "CHANGES_REQUESTED" => Some(ReviewState::ChangesRequested),
        "COMMENTED" => Some(ReviewState::Commented),
        "DISMISSED" => Some(ReviewState::Dismissed),
        _ => None,
    }
}

fn review_decision_from(decision: &str) -> Option<ReviewDecision> {
    match decision {
        "APPROVED" => Some(ReviewDecision::Approved),
        "CHANGES_REQUESTED" => Some(ReviewDecision::ChangesRequested),
        "REVIEW_REQUIRED" => Some(ReviewDecision::ReviewRequired),
        _ => None,
    }
}

fn merge_readiness_from(state: &str) -> Option<MergeReadiness> {
    match state {
        "CLEAN" => Some(MergeReadiness::Ready),
        "BLOCKED" => Some(MergeReadiness::Blocked),
        "BEHIND" => Some(MergeReadiness::Behind),
        "UNSTABLE" => Some(MergeReadiness::Unstable),
        "DIRTY" => Some(MergeReadiness::Conflicts),
        "HAS_HOOKS" => Some(MergeReadiness::Hooks),
        "UNKNOWN" => Some(MergeReadiness::Unknown),
        _ => None,
    }
}

fn check_state_from(state: &str) -> Option<CheckState> {
    match state {
        "SUCCESS" => Some(CheckState::Passed),
        "EXPECTED" | "PENDING" => Some(CheckState::Pending),
        "ERROR" | "FAILURE" => Some(CheckState::Failed),
        _ => None,
    }
}

fn check_context_state(context: &CheckContextInfo) -> Option<CheckState> {
    match context {
        CheckContextInfo::Run {
            status,
            conclusion: _,
            ..
        } if status != "COMPLETED" => Some(CheckState::Pending),
        CheckContextInfo::Run { conclusion, .. } => match conclusion.as_deref() {
            Some("SUCCESS" | "NEUTRAL" | "SKIPPED") => Some(CheckState::Passed),
            Some(
                "ACTION_REQUIRED" | "TIMED_OUT" | "CANCELLED" | "FAILURE" | "STARTUP_FAILURE"
                | "STALE",
            ) => Some(CheckState::Failed),
            None => Some(CheckState::Pending),
            Some(_) => None,
        },
        CheckContextInfo::Status { state, .. } => check_state_from(state),
    }
}

fn summarize_checks(
    state: &str,
    total: u32,
    contexts: &[CheckContextInfo],
) -> Option<CheckSummary> {
    let state = check_state_from(state)?;
    let names = if state == CheckState::Passed {
        Vec::new()
    } else {
        contexts
            .iter()
            .filter(|context| check_context_state(context) == Some(state))
            .map(CheckContextInfo::name)
            .map(str::to_owned)
            .collect()
    };
    Some(CheckSummary {
        state,
        total,
        names,
    })
}

/// Neutral inputs for the pure helpers, so tests need no GraphQL wire shape.
struct ThreadInfo {
    resolved: bool,
    opener: Option<String>,
}

struct ReactionInfo {
    content: String,
    login: String,
}

struct ReviewInfo {
    login: String,
    state: String,
}

enum CheckContextInfo {
    Run {
        name: String,
        status: String,
        conclusion: Option<String>,
    },
    Status {
        name: String,
        state: String,
    },
}

impl CheckContextInfo {
    fn name(&self) -> &str {
        match self {
            CheckContextInfo::Run { name, .. } | CheckContextInfo::Status { name, .. } => name,
        }
    }
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
    Error(RepoError),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RepoError {
    #[error("timeout after 30s")]
    Timeout,
    #[error("{0}")]
    Api(String),
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

/// Whether a GitHub account is a personal user or an organization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountKind {
    User,
    Organization,
}

/// Where `add` should pull the candidate repo list from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListSource {
    /// Everything the token can reach (owned + collaborator + org member),
    /// private included. Used for `--all` and for listing your own account.
    Authenticated,
    /// A specific org's repos (private included when the token is a member).
    Org(String),
    /// A third-party user's public repos — all we can see for someone else.
    PublicUser(String),
}

/// Maps the `type` field of a GitHub account profile to an `AccountKind`.
/// Anything that is not exactly "Organization" is treated as a user.
pub fn account_kind_from_type(profile_type: &str) -> AccountKind {
    if profile_type == "Organization" {
        AccountKind::Organization
    } else {
        AccountKind::User
    }
}

/// Decides which `ListSource` `add` should use.
///
/// - `all`: the `--all` flag was passed.
/// - `username`: the resolved target (`None` when `--all`).
/// - `auth_login`: the login the token authenticates as.
/// - `kind`: whether `username` is a user or org (`None` when `--all`).
pub fn resolve_list_source(
    all: bool,
    username: Option<&str>,
    auth_login: &str,
    kind: Option<AccountKind>,
) -> ListSource {
    if all {
        return ListSource::Authenticated;
    }
    let Some(username) = username else {
        return ListSource::Authenticated;
    };
    if username.eq_ignore_ascii_case(auth_login) {
        return ListSource::Authenticated;
    }
    match kind {
        Some(AccountKind::Organization) => ListSource::Org(username.to_owned()),
        _ => ListSource::PublicUser(username.to_owned()),
    }
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

/// Lists every repo the token can reach — owned, collaborator and
/// organization-member, private included. Backs `add --all` and listing
/// your own account.
pub async fn list_authenticated_repos(crab: &Octocrab) -> Result<Vec<String>> {
    let first_page = crab
        .current()
        .list_repos_for_authenticated_user()
        .visibility("all")
        .affiliation("owner,collaborator,organization_member")
        .per_page(100)
        .send()
        .await
        .context("listing repositories for the authenticated user")?;

    let all_pages = crab
        .all_pages(first_page)
        .await
        .context("paginating authenticated repositories")?;

    let mut names: Vec<String> = all_pages.into_iter().filter_map(|r| r.full_name).collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Lists an org's repos, private included when the token is a member.
pub async fn list_org_repos(crab: &Octocrab, org: &str) -> Result<Vec<String>> {
    let first_page = crab
        .orgs(org)
        .list_repos()
        .repo_type(octocrab::params::repos::Type::All)
        .per_page(100)
        .send()
        .await
        .context("listing organization repositories")?;

    let all_pages = crab
        .all_pages(first_page)
        .await
        .context("paginating organization repositories")?;

    let mut names: Vec<String> = all_pages.into_iter().filter_map(|r| r.full_name).collect();
    names.sort();
    Ok(names)
}

/// The login the token authenticates as, or `None` when unauthenticated
/// (no token / 401) so callers can fall back to public listing.
pub async fn authenticated_login(crab: &Octocrab) -> Result<Option<String>> {
    match crab.current().user().await {
        Ok(user) => Ok(Some(user.login)),
        Err(octocrab::Error::GitHub { source, .. }) if source.status_code.as_u16() == 401 => {
            Ok(None)
        }
        Err(e) => Err(e).context("identifying the authenticated user"),
    }
}

/// Whether `username` is a personal user or an organization.
pub async fn account_kind(crab: &Octocrab, username: &str) -> Result<AccountKind> {
    let profile = crab
        .users(username)
        .profile()
        .await
        .with_context(|| format!("fetching profile for {username}"))?;
    Ok(account_kind_from_type(&profile.r#type))
}

/// Resolves which `ListSource` to use for a concrete target, querying GitHub
/// for the authenticated login and the target's account kind as needed.
pub async fn resolve_source_for(crab: &Octocrab, username: &str) -> Result<ListSource> {
    let auth_login = authenticated_login(crab).await?;
    if let Some(login) = &auth_login
        && login.eq_ignore_ascii_case(username)
    {
        return Ok(ListSource::Authenticated);
    }
    let kind = account_kind(crab, username).await?;
    Ok(resolve_list_source(
        false,
        Some(username),
        auth_login.as_deref().unwrap_or(""),
        Some(kind),
    ))
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
            status: RepoStatus::Error(RepoError::Api(e.to_string())),
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
            requested_reviewers: vec![],
            requested_teams: vec![],
            pr_draft: None,
            pr_extra: None,
        });
    }

    for pr in all_prs {
        let author = pr.user.login.clone();
        let created_at = pr.created_at;
        let pr_draft = pr.draft;
        let requested_reviewers = pr
            .requested_reviewers
            .iter()
            .map(|reviewer| reviewer.login.clone())
            .collect();
        let requested_teams = pr
            .requested_teams
            .iter()
            .map(|team| format!("{owner}/{}", team.slug))
            .collect();
        items.push(RepoItem {
            kind: ItemKind::PullRequest,
            number: pr.number,
            title: pr.title,
            created_at,
            author,
            requested_reviewers,
            requested_teams,
            pr_draft,
            pr_extra: None,
        });
    }

    // Sort: PRs first, then issues; within each group by number descending
    items.sort_by(item_cmp);

    // Review and check enrichment are independent so a token without check
    // access still gets review context. Either query may fail or time out
    // without downgrading the completed REST result.
    if items.iter().any(|i| i.kind == ItemKind::PullRequest) {
        let (extras_result, checks_result) = tokio::join!(
            fetch_pr_extras(crab, owner, name),
            fetch_pr_checks(crab, owner, name),
        );
        let mut extras = extras_result.unwrap_or_default();
        if let Ok(checks) = checks_result {
            for (number, status) in checks {
                let extra = extras.entry(number).or_default();
                extra.checks = status.checks;
                if extra.merge_readiness.is_none() {
                    extra.merge_readiness = status.merge_readiness;
                }
            }
        }
        for item in &mut items {
            if item.kind == ItemKind::PullRequest {
                item.pr_extra = extras.get(&item.number).cloned();
            }
        }
    }

    Ok(items)
}

/// Fetches PR-only review and merge enrichment, newest activity first. Pages
/// until the enrichment deadline and retains every completed page.
async fn fetch_pr_extras(
    crab: &Octocrab,
    owner: &str,
    name: &str,
) -> std::result::Result<HashMap<u64, PrExtra>, octocrab::Error> {
    const QUERY: &str = r#"
query($owner:String!, $name:String!, $cursor:String) {
  repository(owner:$owner, name:$name) {
    pullRequests(states:OPEN, first:100, after:$cursor, orderBy:{field:UPDATED_AT,direction:DESC}) {
      nodes {
        number
        reviewDecision
        mergeStateStatus
        reactions(first:100) { nodes { content user { login } } }
        reviewThreads(first:100) {
          nodes { isResolved comments(first:1) { nodes { author { login } } } }
        }
        latestReviews(first:100) { nodes { author { login } state } }
        reviews(last:100) { nodes { author { login } state } }
      }
      pageInfo { hasNextPage endCursor }
    }
  }
}"#;

    let deadline = tokio::time::Instant::now() + ENRICH_TIMEOUT;
    let mut cursor: Option<String> = None;
    let mut extras = HashMap::new();
    loop {
        let body = serde_json::json!({
            "query": QUERY,
            "variables": { "owner": owner, "name": name, "cursor": cursor },
        });
        // octocrab unwraps the GraphQL `data` envelope, so the response
        // deserializes straight into the repository payload.
        let resp: GqlData = match tokio::time::timeout_at(deadline, crab.graphql(&body)).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(error)) if extras.is_empty() => return Err(error),
            Ok(Err(_)) | Err(_) => break,
        };
        let Some(repo) = resp.repository else {
            break;
        };
        let GqlNodes { nodes, page_info } = repo.pull_requests;
        extras.extend(nodes.into_iter().map(|pr| (pr.number, pr.into())));
        if !page_info.has_next_page {
            break;
        }
        let Some(next_cursor) = page_info.end_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }

    Ok(extras)
}

/// Fetches head-commit check rollups separately so permission errors cannot
/// suppress the review enrichment above. Merge state is repeated here as a
/// cheap fallback when the heavier review query times out. Pages newest
/// activity first and retains every page completed before its deadline.
async fn fetch_pr_checks(
    crab: &Octocrab,
    owner: &str,
    name: &str,
) -> std::result::Result<HashMap<u64, PrCheckExtra>, octocrab::Error> {
    const QUERY: &str = r#"
query($owner:String!, $name:String!, $cursor:String) {
  repository(owner:$owner, name:$name) {
    pullRequests(states:OPEN, first:100, after:$cursor, orderBy:{field:UPDATED_AT,direction:DESC}) {
      nodes {
        number
        mergeStateStatus
        commits(last:1) {
          nodes {
            commit {
              statusCheckRollup {
                state
                contexts(first:100) {
                  totalCount
                  nodes {
                    __typename
                    ... on CheckRun { name status conclusion }
                    ... on StatusContext { context state }
                  }
                }
              }
            }
          }
        }
      }
      pageInfo { hasNextPage endCursor }
    }
  }
}"#;

    let deadline = tokio::time::Instant::now() + ENRICH_TIMEOUT;
    let mut cursor: Option<String> = None;
    let mut checks = HashMap::new();
    loop {
        let body = serde_json::json!({
            "query": QUERY,
            "variables": { "owner": owner, "name": name, "cursor": cursor },
        });
        let resp: GqlCheckData = match tokio::time::timeout_at(deadline, crab.graphql(&body)).await
        {
            Ok(Ok(resp)) => resp,
            Ok(Err(error)) if checks.is_empty() => return Err(error),
            Ok(Err(_)) | Err(_) => break,
        };
        let Some(repo) = resp.repository else {
            break;
        };
        let GqlNodes { nodes, page_info } = repo.pull_requests;
        checks.extend(nodes.into_iter().map(|pr| {
            let number = pr.number;
            (number, pr.into_extra())
        }));
        if !page_info.has_next_page {
            break;
        }
        let Some(next_cursor) = page_info.end_cursor else {
            break;
        };
        cursor = Some(next_cursor);
    }

    Ok(checks)
}

// --- GraphQL wire shapes (deserialized then converted to `PrExtra`) ---
// octocrab returns the unwrapped `data` object, so `GqlData` is the top level.

#[derive(Deserialize)]
struct GqlData {
    repository: Option<GqlRepo>,
}

#[derive(Deserialize)]
struct GqlRepo {
    #[serde(rename = "pullRequests")]
    pull_requests: GqlNodes<GqlPr>,
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct GqlNodes<T> {
    #[serde(default, deserialize_with = "deserialize_nodes")]
    nodes: Vec<T>,
    #[serde(rename = "pageInfo", default)]
    page_info: GqlPageInfo,
}

#[derive(Default, Deserialize)]
struct GqlPageInfo {
    #[serde(rename = "hasNextPage", default)]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

fn deserialize_nodes<'de, D, T>(deserializer: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let nodes = Option::<Vec<Option<T>>>::deserialize(deserializer)?;
    Ok(nodes.unwrap_or_default().into_iter().flatten().collect())
}

impl<T> Default for GqlNodes<T> {
    fn default() -> Self {
        GqlNodes {
            nodes: Vec::new(),
            page_info: GqlPageInfo::default(),
        }
    }
}

#[derive(Deserialize)]
struct GqlPr {
    number: u64,
    #[serde(rename = "reviewDecision")]
    review_decision: Option<String>,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(default)]
    reactions: GqlNodes<GqlReaction>,
    #[serde(rename = "reviewThreads", default)]
    review_threads: GqlNodes<GqlThread>,
    #[serde(rename = "latestReviews", default)]
    latest_reviews: GqlNodes<GqlReview>,
    #[serde(default)]
    reviews: GqlNodes<GqlReview>,
}

#[derive(Deserialize)]
struct GqlReaction {
    content: String,
    user: Option<GqlLogin>,
}

#[derive(Deserialize)]
struct GqlThread {
    #[serde(rename = "isResolved")]
    is_resolved: bool,
    #[serde(default)]
    comments: GqlNodes<GqlComment>,
}

#[derive(Deserialize)]
struct GqlComment {
    author: Option<GqlLogin>,
}

#[derive(Deserialize)]
struct GqlReview {
    author: Option<GqlLogin>,
    state: String,
}

#[derive(Deserialize)]
struct GqlLogin {
    login: String,
}

#[derive(Deserialize)]
struct GqlCheckData {
    repository: Option<GqlCheckRepo>,
}

#[derive(Deserialize)]
struct GqlCheckRepo {
    #[serde(rename = "pullRequests")]
    pull_requests: GqlNodes<GqlCheckPr>,
}

#[derive(Deserialize)]
struct GqlCheckPr {
    number: u64,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(default)]
    commits: GqlNodes<GqlCommitNode>,
}

struct PrCheckExtra {
    checks: Option<CheckSummary>,
    merge_readiness: Option<MergeReadiness>,
}

#[derive(Deserialize)]
struct GqlCommitNode {
    commit: GqlCommit,
}

#[derive(Deserialize)]
struct GqlCommit {
    #[serde(rename = "statusCheckRollup")]
    status_check_rollup: Option<GqlStatusCheckRollup>,
}

#[derive(Deserialize)]
struct GqlStatusCheckRollup {
    state: String,
    contexts: GqlCheckContexts,
}

#[derive(Deserialize)]
struct GqlCheckContexts {
    #[serde(rename = "totalCount")]
    total_count: u32,
    #[serde(default, deserialize_with = "deserialize_nodes")]
    nodes: Vec<GqlCheckContext>,
}

#[derive(Deserialize)]
struct GqlCheckContext {
    #[serde(rename = "__typename")]
    kind: String,
    name: Option<String>,
    status: Option<String>,
    conclusion: Option<String>,
    context: Option<String>,
    state: Option<String>,
}

impl GqlCheckPr {
    fn into_extra(self) -> PrCheckExtra {
        let merge_readiness = self
            .merge_state_status
            .as_deref()
            .and_then(merge_readiness_from);
        let checks = self
            .commits
            .nodes
            .into_iter()
            .next()
            .and_then(|node| node.commit.status_check_rollup)
            .and_then(|rollup| {
                let contexts = rollup
                    .contexts
                    .nodes
                    .into_iter()
                    .filter_map(GqlCheckContext::into_info)
                    .collect::<Vec<_>>();
                summarize_checks(&rollup.state, rollup.contexts.total_count, &contexts)
            });
        PrCheckExtra {
            checks,
            merge_readiness,
        }
    }
}

impl GqlCheckContext {
    fn into_info(self) -> Option<CheckContextInfo> {
        match self.kind.as_str() {
            "CheckRun" => Some(CheckContextInfo::Run {
                name: self.name?,
                status: self.status?,
                conclusion: self.conclusion,
            }),
            "StatusContext" => Some(CheckContextInfo::Status {
                name: self.context?,
                state: self.state?,
            }),
            _ => None,
        }
    }
}

impl From<GqlPr> for PrExtra {
    fn from(pr: GqlPr) -> Self {
        let threads: Vec<ThreadInfo> = pr
            .review_threads
            .nodes
            .into_iter()
            .map(|t| ThreadInfo {
                resolved: t.is_resolved,
                opener: t
                    .comments
                    .nodes
                    .into_iter()
                    .next()
                    .and_then(|c| c.author)
                    .map(|a| a.login),
            })
            .collect();
        let reactions: Vec<ReactionInfo> = pr
            .reactions
            .nodes
            .into_iter()
            .filter_map(|r| {
                r.user.map(|u| ReactionInfo {
                    content: r.content,
                    login: u.login,
                })
            })
            .collect();
        let latest_reviews: Vec<ReviewInfo> = pr
            .latest_reviews
            .nodes
            .into_iter()
            .filter_map(|r| {
                r.author.map(|a| ReviewInfo {
                    login: a.login,
                    state: r.state,
                })
            })
            .collect();
        let review_history: Vec<ReviewInfo> = pr
            .reviews
            .nodes
            .into_iter()
            .filter_map(|r| {
                r.author.map(|a| ReviewInfo {
                    login: a.login,
                    state: r.state,
                })
            })
            .collect();

        let (human_reviews, codex_reviewed) = collapse_reviews(&latest_reviews);
        PrExtra {
            unresolved: unresolved_by_author(&threads),
            codex: codex_reaction(&reactions),
            codex_reviewed,
            reviews: human_reviews,
            prior_reviewers: prior_reviewers(&review_history),
            decision: pr.review_decision.as_deref().and_then(review_decision_from),
            checks: None,
            merge_readiness: pr
                .merge_state_status
                .as_deref()
                .and_then(merge_readiness_from),
        }
    }
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
            requested_reviewers: vec![],
            requested_teams: vec![],
            pr_draft: None,
            pr_extra: None,
        }
    }

    fn thread(resolved: bool, opener: Option<&str>) -> ThreadInfo {
        ThreadInfo {
            resolved,
            opener: opener.map(str::to_owned),
        }
    }

    fn reaction(content: &str, login: &str) -> ReactionInfo {
        ReactionInfo {
            content: content.into(),
            login: login.into(),
        }
    }

    fn review(login: &str, state: &str) -> ReviewInfo {
        ReviewInfo {
            login: login.into(),
            state: state.into(),
        }
    }

    fn check_run(name: &str, status: &str, conclusion: Option<&str>) -> CheckContextInfo {
        CheckContextInfo::Run {
            name: name.into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_owned),
        }
    }

    fn status_context(name: &str, state: &str) -> CheckContextInfo {
        CheckContextInfo::Status {
            name: name.into(),
            state: state.into(),
        }
    }

    #[test]
    fn is_codex_actor_matches_bot_and_bare_login() {
        assert!(is_codex_actor("chatgpt-codex-connector[bot]"));
        assert!(is_codex_actor("chatgpt-codex-connector"));
        assert!(!is_codex_actor("coderabbitai[bot]"));
        assert!(!is_codex_actor("milesibastos"));
    }

    #[test]
    fn unresolved_by_author_counts_and_orders() {
        let threads = [
            thread(true, Some("milesibastos")),
            thread(false, Some("chatgpt-codex-connector")),
            thread(false, Some("chatgpt-codex-connector")),
            thread(false, Some("alvarolopes")),
            thread(false, None),
        ];
        assert_eq!(
            unresolved_by_author(&threads),
            vec![
                ("chatgpt-codex-connector".to_owned(), 2),
                ("alvarolopes".to_owned(), 1),
            ]
        );
    }

    #[test]
    fn unresolved_by_author_empty_when_all_resolved() {
        let threads = [thread(true, Some("a")), thread(true, Some("b"))];
        assert!(unresolved_by_author(&threads).is_empty());
    }

    #[test]
    fn codex_reaction_prefers_lgtm_over_reviewing() {
        let reactions = [
            reaction("EYES", "chatgpt-codex-connector[bot]"),
            reaction("THUMBS_UP", "chatgpt-codex-connector[bot]"),
        ];
        assert_eq!(codex_reaction(&reactions), Some(CodexReaction::Lgtm));
    }

    #[test]
    fn codex_reaction_reviewing_when_only_eyes() {
        let reactions = [reaction("EYES", "chatgpt-codex-connector[bot]")];
        assert_eq!(codex_reaction(&reactions), Some(CodexReaction::Reviewing));
    }

    #[test]
    fn codex_reaction_ignores_non_codex_and_other_content() {
        let reactions = [
            reaction("THUMBS_UP", "milesibastos"),
            reaction("HEART", "chatgpt-codex-connector[bot]"),
        ];
        assert_eq!(codex_reaction(&reactions), None);
    }

    #[test]
    fn collapse_reviews_excludes_codex_but_flags_it() {
        let reviews = [
            review("chatgpt-codex-connector", "COMMENTED"),
            review("milesibastos", "APPROVED"),
            review("alvarolopes", "CHANGES_REQUESTED"),
        ];
        let (humans, codex_reviewed) = collapse_reviews(&reviews);
        assert!(codex_reviewed);
        assert_eq!(
            humans,
            vec![
                ("milesibastos".to_owned(), ReviewState::Approved),
                ("alvarolopes".to_owned(), ReviewState::ChangesRequested),
            ]
        );
    }

    #[test]
    fn collapse_reviews_no_codex_flag_when_absent() {
        let reviews = [review("milesibastos", "COMMENTED")];
        let (humans, codex_reviewed) = collapse_reviews(&reviews);
        assert!(!codex_reviewed);
        assert_eq!(
            humans,
            vec![("milesibastos".to_owned(), ReviewState::Commented)]
        );
    }

    #[test]
    fn prior_reviewers_are_submitted_and_unique_case_insensitively() {
        let reviews = [
            review("pending-reviewer", "PENDING"),
            review("anbillin", "COMMENTED"),
            review("mdo2", "APPROVED"),
            review("AnBillin", "COMMENTED"),
        ];

        assert_eq!(
            prior_reviewers(&reviews),
            vec!["anbillin".to_owned(), "mdo2".to_owned()]
        );
    }

    #[test]
    fn review_decision_from_maps_known_values() {
        assert_eq!(
            review_decision_from("APPROVED"),
            Some(ReviewDecision::Approved)
        );
        assert_eq!(
            review_decision_from("CHANGES_REQUESTED"),
            Some(ReviewDecision::ChangesRequested)
        );
        assert_eq!(
            review_decision_from("REVIEW_REQUIRED"),
            Some(ReviewDecision::ReviewRequired)
        );
        assert_eq!(review_decision_from(""), None);
    }

    #[test]
    fn merge_readiness_from_maps_github_states() {
        assert_eq!(merge_readiness_from("CLEAN"), Some(MergeReadiness::Ready));
        assert_eq!(
            merge_readiness_from("BLOCKED"),
            Some(MergeReadiness::Blocked)
        );
        assert_eq!(merge_readiness_from("BEHIND"), Some(MergeReadiness::Behind));
        assert_eq!(
            merge_readiness_from("UNSTABLE"),
            Some(MergeReadiness::Unstable)
        );
        assert_eq!(
            merge_readiness_from("DIRTY"),
            Some(MergeReadiness::Conflicts)
        );
        assert_eq!(
            merge_readiness_from("HAS_HOOKS"),
            Some(MergeReadiness::Hooks)
        );
        assert_eq!(
            merge_readiness_from("UNKNOWN"),
            Some(MergeReadiness::Unknown)
        );
        assert_eq!(merge_readiness_from("NEW_STATE"), None);
    }

    #[test]
    fn failed_checks_include_runs_and_legacy_statuses() {
        let contexts = [
            check_run("cargo-test", "COMPLETED", Some("FAILURE")),
            check_run("clippy", "COMPLETED", Some("CANCELLED")),
            check_run("format", "COMPLETED", Some("SUCCESS")),
            status_context("coverage", "ERROR"),
        ];

        assert_eq!(
            summarize_checks("FAILURE", 4, &contexts),
            Some(CheckSummary {
                state: CheckState::Failed,
                total: 4,
                names: vec!["cargo-test".into(), "clippy".into(), "coverage".into()],
            })
        );
    }

    #[test]
    fn pending_checks_include_queued_runs_and_expected_statuses() {
        let contexts = [
            check_run("cargo-test", "IN_PROGRESS", None),
            check_run("clippy", "QUEUED", None),
            status_context("deploy", "EXPECTED"),
            status_context("coverage", "SUCCESS"),
        ];

        assert_eq!(
            summarize_checks("PENDING", 4, &contexts),
            Some(CheckSummary {
                state: CheckState::Pending,
                total: 4,
                names: vec!["cargo-test".into(), "clippy".into(), "deploy".into()],
            })
        );
    }

    #[test]
    fn passed_checks_retain_total_without_names() {
        let contexts = [
            check_run("cargo-test", "COMPLETED", Some("SUCCESS")),
            check_run("docs", "COMPLETED", Some("SKIPPED")),
        ];

        assert_eq!(
            summarize_checks("SUCCESS", 2, &contexts),
            Some(CheckSummary {
                state: CheckState::Passed,
                total: 2,
                names: vec![],
            })
        );
    }

    #[test]
    fn check_conclusions_map_to_github_rollup_groups() {
        for conclusion in [
            "ACTION_REQUIRED",
            "TIMED_OUT",
            "CANCELLED",
            "FAILURE",
            "STARTUP_FAILURE",
            "STALE",
        ] {
            assert_eq!(
                check_context_state(&check_run("ci", "COMPLETED", Some(conclusion))),
                Some(CheckState::Failed)
            );
        }
        for conclusion in ["SUCCESS", "NEUTRAL", "SKIPPED"] {
            assert_eq!(
                check_context_state(&check_run("ci", "COMPLETED", Some(conclusion))),
                Some(CheckState::Passed)
            );
        }
    }

    #[test]
    fn check_graphql_shape_tolerates_null_connections_and_nodes() {
        let data: GqlCheckData = serde_json::from_value(serde_json::json!({
            "repository": {
                "pullRequests": {
                    "pageInfo": {
                        "hasNextPage": true,
                        "endCursor": "next-page"
                    },
                    "nodes": [
                        null,
                        {
                            "number": 7,
                            "mergeStateStatus": "DIRTY",
                            "commits": {
                                "nodes": [
                                    null,
                                    {
                                        "commit": {
                                            "statusCheckRollup": {
                                                "state": "FAILURE",
                                                "contexts": {
                                                    "totalCount": 1,
                                                    "nodes": [
                                                        null,
                                                        {
                                                            "__typename": "CheckRun",
                                                            "name": "cargo-test",
                                                            "status": "COMPLETED",
                                                            "conclusion": "FAILURE"
                                                        }
                                                    ]
                                                }
                                            }
                                        }
                                    }
                                ]
                            }
                        },
                        {
                            "number": 8,
                            "mergeStateStatus": "CLEAN",
                            "commits": { "nodes": null }
                        }
                    ]
                }
            }
        }))
        .unwrap();

        let connection = data.repository.unwrap().pull_requests;
        assert!(connection.page_info.has_next_page);
        assert_eq!(
            connection.page_info.end_cursor.as_deref(),
            Some("next-page")
        );
        let mut nodes = connection.nodes;
        assert_eq!(nodes.len(), 2);
        let first = nodes.remove(0).into_extra();
        assert_eq!(first.merge_readiness, Some(MergeReadiness::Conflicts));
        assert_eq!(
            first.checks,
            Some(CheckSummary {
                state: CheckState::Failed,
                total: 1,
                names: vec!["cargo-test".into()],
            })
        );
        let second = nodes.remove(0).into_extra();
        assert_eq!(second.merge_readiness, Some(MergeReadiness::Ready));
        assert_eq!(second.checks, None);
    }

    #[test]
    fn account_kind_organization() {
        assert_eq!(
            account_kind_from_type("Organization"),
            AccountKind::Organization
        );
    }

    #[test]
    fn account_kind_user() {
        assert_eq!(account_kind_from_type("User"), AccountKind::User);
    }

    #[test]
    fn account_kind_unknown_defaults_to_user() {
        assert_eq!(account_kind_from_type("Bot"), AccountKind::User);
    }

    #[test]
    fn all_flag_lists_authenticated() {
        assert_eq!(
            resolve_list_source(true, None, "me", None),
            ListSource::Authenticated
        );
    }

    #[test]
    fn own_username_lists_authenticated() {
        assert_eq!(
            resolve_list_source(false, Some("me"), "me", Some(AccountKind::User)),
            ListSource::Authenticated
        );
    }

    #[test]
    fn own_username_is_case_insensitive() {
        assert_eq!(
            resolve_list_source(false, Some("ME"), "me", Some(AccountKind::User)),
            ListSource::Authenticated
        );
    }

    #[test]
    fn org_target_lists_org_repos() {
        assert_eq!(
            resolve_list_source(false, Some("acme"), "me", Some(AccountKind::Organization)),
            ListSource::Org("acme".to_owned())
        );
    }

    #[test]
    fn third_party_user_lists_public_only() {
        assert_eq!(
            resolve_list_source(false, Some("octocat"), "me", Some(AccountKind::User)),
            ListSource::PublicUser("octocat".to_owned())
        );
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
