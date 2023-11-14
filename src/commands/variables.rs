use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud::{client::Client as CloudClient, CloudClientInterface};
use serde::Deserialize;
use serde_json::from_str;
use spin_common::arg_parser::parse_kv;
use uuid::Uuid;

use crate::commands::client_and_app_id;
use crate::opts::*;

#[derive(Deserialize)]
pub(crate) struct Variable {
    pub key: String,
}

/// Manage Spin application variables
#[derive(Parser, Debug)]
#[clap(about = "Manage Spin application variables")]
pub enum VariablesCommand {
    /// Set variables
    Set(SetCommand),
    /// Delete variables
    Delete(DeleteCommand),
    /// List all variables of an application
    List(ListCommand),
}

#[derive(Parser, Debug)]
pub struct SetCommand {
    /// Variable pair to set
    #[clap(parse(try_from_str = parse_kv))]
    pub variables_to_set: Vec<(String, String)>,
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct DeleteCommand {
    /// Variable pair to set
    pub variables_to_delete: Vec<String>,
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
        env = DEPLOYMENT_ENV_NAME_ENV,
        hidden = true
    )]
    pub deployment_env_id: Option<String>,

    /// Name of Spin app
    #[clap(name = "app", long = "app")]
    pub app: String,
}

impl VariablesCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            Self::Set(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.common.app)
                        .await?;
                set_variables(&client, app_id, &cmd.variables_to_set).await?;
            }
            Self::Delete(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.common.app)
                        .await?;
                delete_variables(&client, app_id, &cmd.variables_to_delete).await?;
            }
            Self::List(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.common.app)
                        .await?;
                let var_names = get_variables(&client, app_id).await?;
                for v in var_names {
                    println!("{}", v.key);
                }
            }
        }
        Ok(())
    }
}

pub(crate) async fn set_variables(
    client: &CloudClient,
    app_id: Uuid,
    variables: &[(String, String)],
) -> Result<()> {
    for var in variables {
        CloudClient::add_variable_pair(client, app_id, var.0.to_owned(), var.1.to_owned())
            .await
            .with_context(|| format!("Problem creating variable {}", var.0))?;
    }
    Ok(())
}

pub(crate) async fn delete_variables(
    client: &CloudClient,
    app_id: Uuid,
    variables: &[String],
) -> Result<()> {
    for var in variables {
        CloudClient::delete_variable_pair(client, app_id, var.to_owned())
            .await
            .with_context(|| format!("Problem deleting variable {var}"))?;
    }
    Ok(())
}

async fn get_variables_json(client: &CloudClient, app_id: Uuid) -> Result<Vec<String>> {
    let vars = CloudClient::get_variable_pairs(client, app_id)
        .await
        .context("Problem listing variables")?;
    Ok(vars)
}

pub(crate) async fn get_variables(client: &CloudClient, app_id: Uuid) -> Result<Vec<Variable>> {
    let vars = get_variables_json(client, app_id).await?;
    let var_names = vars
        .iter()
        .map(|var| from_str(var))
        .collect::<Result<Vec<Variable>, _>>()
        .context("could not parse variable")?;
    Ok(var_names)
}
