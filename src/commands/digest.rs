use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use octocrab::Octocrab;

use crate::{config, display, github};

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
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let futures: Vec<_> = cfg
        .repos
        .iter()
        .map(|repo| github::fetch_repo_items(crab, repo))
        .collect();

    let results = futures::future::join_all(futures).await;

    spinner.finish_and_clear();

    let digest = display::render_digest(&results);
    print!("{}", digest);

    Ok(())
}
