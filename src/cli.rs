use clap::{Parser, Subcommand};
use std::path::PathBuf;

use crate::config::FilterMode;

#[derive(Parser)]
#[command(
    name = "ghpending",
    about = "Digest of open issues and PRs across watched repos"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
    /// Color theme (default, evangelion, nerv)
    #[arg(long, global = true)]
    pub theme: Option<String>,
    /// Use a specific config file, bypassing local/global discovery
    #[arg(long, global = true, value_name = "PATH")]
    pub config: Option<PathBuf>,
    /// Show items authored by this GitHub login (repeatable)
    #[arg(long = "author", value_name = "LOGIN")]
    pub authors: Vec<String>,
    /// Show PRs currently awaiting this user or team:ORG/SLUG (repeatable)
    #[arg(long = "review-requested", value_name = "LOGIN|team:ORG/SLUG")]
    pub review_requested: Vec<String>,
    /// Match any or all configured filter roles
    #[arg(long = "match", value_enum)]
    pub filter_mode: Option<FilterMode>,
}

impl Cli {
    pub fn has_digest_filter_args(&self) -> bool {
        !self.authors.is_empty() || !self.review_requested.is_empty() || self.filter_mode.is_some()
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Pick repos from a GitHub user/org to track
    Add {
        /// GitHub user/org to list repos from; replaces the saved one.
        /// Lists private repos too when it's your own account or an org you
        /// belong to (needs a GITHUB_TOKEN with `repo` scope).
        #[arg(long, conflicts_with = "all")]
        user: Option<String>,
        /// List every repo your token can reach (owned, collaborator and
        /// org-member), private included, ignoring the saved user.
        #[arg(long)]
        all: bool,
    },
    /// Remove repos from the watch list
    Rm,
    /// Print all tracked repos
    List,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repeatable_digest_filters() {
        let cli = Cli::try_parse_from([
            "ghpending",
            "--author",
            "alice",
            "--author",
            "bob",
            "--review-requested",
            "team:owner/core",
            "--match",
            "all",
        ])
        .unwrap();

        assert_eq!(cli.authors, ["alice", "bob"]);
        assert_eq!(cli.review_requested, ["team:owner/core"]);
        assert_eq!(cli.filter_mode, Some(FilterMode::All));
        assert!(cli.command.is_none());
    }
}
