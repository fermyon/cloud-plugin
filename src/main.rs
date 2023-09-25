mod commands;
mod opts;

use anyhow::{Error, Result};
use clap::{FromArgMatches, Parser};
use commands::{
    apps::AppsCommand, deploy::DeployCommand, login::LoginCommand, sqlite::SqliteCommand,
    variables::VariablesCommand,
};
use semver::BuildMetadata;

/// Returns build information, similar to: 0.1.0 (2be4034 2022-03-31).
const VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("VERGEN_GIT_SHA"),
    " ",
    env!("VERGEN_GIT_COMMIT_DATE"),
    ")"
);

#[derive(Parser)]
#[clap(author, version = VERSION, about, long_about = None)]
#[clap(propagate_version = true)]
enum CloudCli {
    /// Manage applications deployed to Fermyon Cloud
    #[clap(subcommand, alias = "app")]
    Apps(AppsCommand),
    /// Package and upload an application to the Fermyon Cloud.
    Deploy(DeployCommand),
    /// Login to Fermyon Cloud
    Login(LoginCommand),
    /// Manage Spin application variables
    #[clap(subcommand, alias = "vars")]
    Variables(VariablesCommand),
    /// Manage Fermyon Cloud NoOps SQL databases
    #[clap(subcommand)]
    Sqlite(SqliteCommand),
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let mut app = CloudCli::clap();
    // Plugin should always be invoked from Spin so set binary name accordingly
    app.set_bin_name("spin cloud");
    let matches = app.get_matches();
    let cli = CloudCli::from_arg_matches(&matches)?;

    match cli {
        CloudCli::Apps(cmd) => cmd.run().await,
        CloudCli::Deploy(cmd) => cmd.run().await,
        CloudCli::Login(cmd) => cmd.run().await,
        CloudCli::Variables(cmd) => cmd.run().await,
        CloudCli::Sqlite(cmd) => cmd.run().await,
    }
}

pub(crate) fn parse_buildinfo(buildinfo: &str) -> Result<BuildMetadata> {
    Ok(BuildMetadata::new(buildinfo)?)
}
