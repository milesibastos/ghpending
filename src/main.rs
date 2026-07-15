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

    let (cfg_path, cfg_source) = config::resolve_path(cli.config.as_deref())?;
    if cfg_source != config::ConfigSource::Global {
        eprintln!("using config {}", cfg_path.display());
    }
    let cfg = config::load_from(&cfg_path)?;

    let env_specific = std::env::var("GHPENDING_THEME").ok();
    let env_generic = std::env::var("TCLOCK_WIDGET_THEME").ok();
    let theme_name = theme::resolve_name(
        cli.theme.as_deref(),
        env_specific.as_deref(),
        env_generic.as_deref(),
        cfg.theme.as_deref(),
    );
    let resolved_theme = match Theme::by_name(&theme_name) {
        Some(t) => t,
        None => bail!(
            "unknown theme: {} (available: {})",
            theme_name,
            theme::THEME_NAMES.join(", ")
        ),
    };

    let crab = github_client::build()?;

    match &cli.command {
        Some(Commands::List) => commands::list::run(&cfg_path)?,
        Some(Commands::Rm) => commands::remove::run(&cfg_path)?,
        Some(Commands::Add { user, all }) => {
            commands::add::run(&crab, user.clone(), *all, &cfg_path).await?
        }
        None => commands::digest::run(&crab, &resolved_theme, &cfg_path).await?,
    }

    Ok(())
}
