mod commands;
mod manifest;
mod opts;
use anyhow::{Error, Result};
use clap::Parser;
use commands::{deploy::DeployCommand, login::LoginCommand, variables::VariablesCommand};
use semver::BuildMetadata;

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

pub(crate) fn parse_buildinfo(buildinfo: &str) -> Result<BuildMetadata> {
    Ok(BuildMetadata::new(buildinfo)?)
}
