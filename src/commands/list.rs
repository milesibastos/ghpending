use crate::config;
use anyhow::Result;

pub fn run() -> Result<()> {
    let cfg = config::load()?;
    if cfg.repos.is_empty() {
        println!("No repos tracked. Run `ghpending add` to get started.");
    } else {
        for repo in &cfg.repos {
            println!("{}", repo);
        }
    }
    Ok(())
}
