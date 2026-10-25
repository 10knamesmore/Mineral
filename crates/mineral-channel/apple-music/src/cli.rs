//! Explicit login using hidden terminal input or a piped token.

use std::io::{self, IsTerminal};

use clap::{Args, Subcommand};

use crate::api::Api;
use crate::credential::{from_token, save_login};
use crate::{AppleMusicConfig, Error, Result};

/// Apple Music account commands.
#[derive(Debug, Args)]
pub struct AppleMusicCli {
    /// Source operation.
    #[command(subcommand)]
    pub command: AppleMusicCommand,
}

/// Account operations; tokens are never accepted as command-line arguments.
#[derive(Debug, Subcommand)]
pub enum AppleMusicCommand {
    /// Read a media-user-token from hidden terminal input or piped stdin.
    Login,
}

/// Validates the supplied token before replacing the saved credential.
pub async fn run(cli: AppleMusicCli, config: &AppleMusicConfig) -> Result<()> {
    match cli.command {
        AppleMusicCommand::Login => {
            tokio::select! {
                result = login(config) => result,
                _ = tokio::signal::ctrl_c() => Err(Error::Cancelled),
            }
        }
    }
}

async fn login(config: &AppleMusicConfig) -> Result<()> {
    eprintln!("Enter the Apple Music media-user-token (hidden in a terminal):");
    let token = tokio::task::spawn_blocking(read_token).await??;
    let auth = from_token(&token)?;
    Api::new(config, auth.clone())?.session().await?;
    let path = save_login(auth).await?;
    println!(
        "Apple Music login saved to {}. Restart the daemon to load it.",
        path.display()
    );
    Ok(())
}

fn read_token() -> Result<String> {
    if io::stdin().is_terminal() {
        Ok(rpassword::read_password()?)
    } else {
        let mut token = String::new();
        io::stdin().read_line(&mut token)?;
        Ok(token)
    }
}
