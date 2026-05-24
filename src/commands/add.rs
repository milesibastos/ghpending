use anyhow::Result;
use inquire::{MultiSelect, Text};
use octocrab::Octocrab;

use crate::{config, github};

pub async fn run(crab: &Octocrab) -> Result<()> {
    let mut cfg = config::load()?;

    let username = match cfg.user.clone() {
        Some(u) => u,
        None => {
            let u = Text::new("GitHub username or org to list repos from:").prompt()?;
            let u = u.trim().to_string();
            cfg.user = Some(u.clone());
            config::save(&cfg)?;
            u
        }
    };

    let found = github::list_user_repos(crab, &username).await?;
    if found.is_empty() {
        println!("No public repos found for: {}", username);
        return Ok(());
    }

    let already: std::collections::HashSet<&str> = cfg.repos.iter().map(|s| s.as_str()).collect();

    let defaults: Vec<usize> = found
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            if already.contains(r.as_str()) {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    let selected = MultiSelect::new("Select repos to track:", found)
        .with_default(&defaults)
        .prompt()?;

    for repo in selected {
        if !cfg.repos.contains(&repo) {
            cfg.repos.push(repo);
        }
    }
    cfg.repos.sort();
    config::save(&cfg)?;
    println!("Saved. Tracking {} repo(s) total.", cfg.repos.len());
    Ok(())
}
