use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::client::Client as CloudClient;
use cloud_openapi::models::Database;
use dialoguer::Input;

use crate::commands::{create_cloud_client, get_app_id_cloud};
use crate::opts::*;

/// Manage Fermyon Cloud SQLite databases
#[derive(Parser, Debug)]
#[clap(about = "Manage Fermyon Cloud SQLite databases")]
pub enum SqliteCommand {
    /// Delete a SQLite database
    Delete(DeleteCommand),
    /// Execute SQL against a SQLite database
    Execute(ExecuteCommand),
    /// List all SQLite databases of a user
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
    name: String,

    ///Statement to execute
    statement: String,

    /// Name of Spin app
    #[clap(name = "app", long = "app")]
    pub app: String,

    #[clap(flatten)]
    common: CommonArgs,
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
            Self::Delete(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let list = CloudClient::get_databases(&client, None)
                    .await
                    .context("Problem fetching databases")?;
                if !list.iter().any(|d| d.name == cmd.name) {
                    anyhow::bail!("No database found with name \"{}\"", cmd.name);
                }
                // TODO: Fail if apps exist that are currently using a database
                if cmd.yes || prompt_delete_database(&cmd.name)? {
                    CloudClient::delete_database(&client, cmd.name.clone())
                        .await
                        .with_context(|| format!("Problem deleting database {}", cmd.name))?;
                    println!("Database \"{}\" deleted", cmd.name);
                }
            }
            Self::Execute(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let list = CloudClient::get_databases(&client, None)
                    .await
                    .context("Problem fetching databases")?;
                if !list.iter().any(|d| d.name == cmd.name) {
                    anyhow::bail!("No database found with name \"{}\"", cmd.name);
                }
                let app_id = get_app_id_cloud(&client, &cmd.app).await?;
                println!("Executing SQL: {}", cmd.statement);
                CloudClient::execute_sql(&client, app_id, cmd.name, cmd.statement)
                    .await
                    .context("Problem executing SQL")?;
            }
            Self::List(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let list = CloudClient::get_databases(&client, None)
                    .await
                    .context("Problem listing databases")?;
                print_databases(list);
            }
        }
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
