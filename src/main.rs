mod commands;
mod manifest;
mod opts;
use anyhow::{anyhow, Error, Result};
use clap::Parser;
use commands::{deploy::DeployCommand, login::LoginCommand, variables::VariablesCommand};
use semver::BuildMetadata;
use spin_bindle::PublishError;
use std::path::Path;

#[derive(Parser)]
#[clap(author, version, about, long_about = None)]
#[clap(propagate_version = true)]
enum CloudCli {
    /// Package and upload an application to the Fermyon Cloud.
    Deploy(DeployCommand),
    /// Login to Fermyon Cloud
    Login(LoginCommand),
    /// Manage Spin application variables
    #[clap(subcommand, alias = "vars", hide = true)]
    Variables(VariablesCommand),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let cli = CloudCli::parse();

    match cli {
        CloudCli::Deploy(cmd) => cmd.run().await,
        CloudCli::Login(cmd) => cmd.run().await,
        CloudCli::Variables(cmd) => cmd.run().await,
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
