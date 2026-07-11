mod cli;
mod commands;
mod config;
mod display;
mod format;
mod github;
mod github_client;
mod theme;

use anyhow::{Result, bail};
use clap::Parser;
use cli::{Cli, Commands};
use theme::Theme;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let cfg = config::load()?;

    let theme_name = cli
        .theme
        .as_deref()
        .or(cfg.theme.as_deref())
        .unwrap_or("default");
    let resolved_theme = match Theme::by_name(theme_name) {
        Some(t) => t,
        None => bail!(
            "unknown theme: {} (available: {})",
            theme_name,
            theme::THEME_NAMES.join(", ")
        ),
    };

    let crab = github_client::build()?;

    match &cli.command {
        Some(Commands::List) => commands::list::run()?,
        Some(Commands::Rm) => commands::remove::run()?,
        Some(Commands::Add { user, all }) => commands::add::run(&crab, user.clone(), *all).await?,
        None => commands::digest::run(&crab, &resolved_theme).await?,
    }

    Ok(())
}
