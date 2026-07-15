use crate::config;
use anyhow::Result;
use std::path::Path;

pub fn run(cfg_path: &Path) -> Result<()> {
    let cfg = config::load_from(cfg_path)?;
    if cfg.repos.is_empty() {
        println!("No repos tracked. Run `ghpending add` to get started.");
    } else {
        for repo in &cfg.repos {
            println!("{repo}");
        }
    }
    Ok(())
}
