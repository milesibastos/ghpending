mod cli;
mod commands;
mod config;
mod display;
mod format;
mod github;
mod github_client;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let crab = github_client::build()?;

    match &cli.command {
        Some(Commands::List) => commands::list::run()?,
        Some(Commands::Rm) => commands::remove::run()?,
        Some(Commands::Add { user, all }) => commands::add::run(&crab, user.clone(), *all).await?,
        None => commands::digest::run(&crab).await?,
    }

    Ok(())
}
