use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::client::Client as CloudClient;
use cloud_openapi::models::Database;
use dialoguer::Input;

use crate::commands::create_cloud_client;
use crate::opts::*;

/// Manage Fermyon Cloud NoOps SQL databases
#[derive(Parser, Debug)]
#[clap(about = "Manage Fermyon Cloud SQLite databases")]
pub enum SqliteCommand {
    /// Delete a NoOps SQL database
    Delete(DeleteCommand),
    /// Execute SQLite statements against a NoOps SQL database
    Execute(ExecuteCommand),
    /// List all NoOps SQL databases of a user
    List(ListCommand),
}

#[derive(Parser, Debug)]
pub struct DeleteCommand {
    /// Name of database to delete
    name: String,

    /// Skips prompt to confirm deletion of database
    #[clap(short = 'y', long = "yes", takes_value = false)]
    yes: bool,

    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct ExecuteCommand {
    /// Name of database to execute against
    #[clap(value_parser = clap::builder::ValueParser::new(disallow_empty))]
    name: String,

    ///Statement to execute
    #[clap(value_parser = clap::builder::ValueParser::new(disallow_empty))]
    statement: String,

    #[clap(flatten)]
    common: CommonArgs,
}

fn disallow_empty(statement: &str) -> anyhow::Result<String> {
    if statement.trim().is_empty() {
        anyhow::bail!("cannot be empty");
    }
    return Ok(statement.trim().to_owned());
}

#[derive(Parser, Debug)]
pub struct ListCommand {
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Debug, Default, Args)]
struct CommonArgs {
    /// Deploy to the Fermyon instance saved under the specified name.
    /// If omitted, Spin deploys to the default unnamed instance.
    #[clap(
        name = "environment-name",
        long = "environment-name",
        env = DEPLOYMENT_ENV_NAME_ENV
    )]
    pub deployment_env_id: Option<String>,
}

impl SqliteCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Delete(cmd) => cmd.run().await,
            Self::Execute(cmd) => cmd.run().await,
            Self::List(cmd) => cmd.run().await,
        }
    }
}

impl DeleteCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = CloudClient::get_databases(&client, None)
            .await
            .context("Problem fetching databases")?;
        if !list.iter().any(|d| d.name == self.name) {
            anyhow::bail!("No database found with name \"{}\"", self.name);
        }
        // TODO: Fail if apps exist that are currently using a database
        if self.yes || prompt_delete_database(&self.name)? {
            CloudClient::delete_database(&client, self.name.clone())
                .await
                .with_context(|| format!("Problem deleting database {}", self.name))?;
            println!("Database \"{}\" deleted", self.name);
        }
        Ok(())
    }
}

impl ExecuteCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = CloudClient::get_databases(&client, None)
            .await
            .context("Problem fetching databases")?;
        if !list.iter().any(|d| d.name == self.name) {
            anyhow::bail!("No database found with name \"{}\"", self.name);
        }
        let statement = if let Some(path) = self.statement.strip_prefix('@') {
            std::fs::read_to_string(path)
                .with_context(|| format!("could not read sql file at '{path}'"))?
        } else {
            self.statement
        };
        CloudClient::execute_sql(&client, self.name, statement)
            .await
            .context("Problem executing SQL")?;
        Ok(())
    }
}

impl ListCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let list = CloudClient::get_databases(&client, None)
            .await
            .context("Problem listing databases")?;
        print_databases(list);
        Ok(())
    }
}

fn print_databases(databases: Vec<Database>) {
    if databases.is_empty() {
        println!("No databases");
        return;
    }
    terminal::step!("Databases", "({})", databases.len());
    for d in databases {
        let default_str = if d.default { " (default)" } else { "" };
        println!("{}{default_str}", d.name);
    }
}

fn prompt_delete_database(database_name: &str) -> std::io::Result<bool> {
    let mut input = Input::<String>::new();
    let prompt =
        format!("The action is irreversible. Please type \"{database_name}\" for confirmation",);
    input.with_prompt(prompt);
    let answer = input.interact_text()?;
    if answer != database_name {
        println!("Invalid confirmation. Will not delete database.");
        Ok(false)
    } else {
        println!("Deleting database ...");
        Ok(true)
    }
}
