use anyhow::Result;
use inquire::MultiSelect;

use crate::config;

pub fn run() -> Result<()> {
    let mut cfg = config::load()?;
    if cfg.repos.is_empty() {
        println!("No repos tracked.");
        return Ok(());
    }

    let to_remove = MultiSelect::new(
        "Uncheck repos to remove (space to toggle, enter to confirm):",
        cfg.repos.clone(),
    )
    .prompt()?;

    let remove_set: std::collections::HashSet<&str> =
        to_remove.iter().map(|s| s.as_str()).collect();
    cfg.repos.retain(|r| !remove_set.contains(r.as_str()));
    config::save(&cfg)?;

    if to_remove.is_empty() {
        println!("Nothing removed.");
    } else {
        println!("Removed {} repo(s).", to_remove.len());
    }
    Ok(())
}
