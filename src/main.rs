mod commands;
mod opts;
use anyhow::{anyhow, Error, Result};
use clap::{Parser, Subcommand};
use commands::{deploy::DeployCommand, login::LoginCommand};
use semver::BuildMetadata;
use spin_bindle::PublishError;
use std::path::Path;

pub use crate::opts::HELP_ARGS_ONLY_TRIGGER_TYPE;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Package and upload an application to the Fermyon Cloud.
    Deploy(DeployCommand),
    /// Login to the Fermyon Platform.
    Login(LoginCommand),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = Cli::parse();

    // You can check for the existence of subcommands, and if found use their
    // matches just as you would the top level cmd
    match cli.command {
        Commands::Deploy(cmd) => cmd.run().await,
        Commands::Login(cmd) => cmd.run().await,
    }
}

pub(crate) fn push_all_failed_msg(path: &Path, server_url: &str) -> String {
    format!(
        "Failed to push bindle from '{}' to the server at '{}'",
        path.display(),
        server_url
    )
}

pub(crate) fn wrap_prepare_bindle_error(err: PublishError) -> anyhow::Error {
    match err {
        PublishError::MissingBuildArtifact(_) => {
            anyhow!("{}\n\nPlease try to run `spin build` first", err)
        }
        e => anyhow!(e),
    }
}

pub(crate) fn parse_buildinfo(buildinfo: &str) -> Result<BuildMetadata> {
    Ok(BuildMetadata::new(buildinfo)?)
}
