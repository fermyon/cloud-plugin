use crate::commands::{client_and_app_id, create_cloud_client};
use crate::opts::*;
use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud_openapi::models::{AppItemPage, DomainValidationStatus};

#[derive(Parser, Debug)]
#[clap(about = "Manage applications deployed to Fermyon Cloud")]
pub enum AppsCommand {
    /// List all the apps deployed in Fermyon Cloud
    List(ListCommand),
    /// Delete an app deployed in Fermyon Cloud
    Delete(DeleteCommand),
    /// Get details about a deployed app in Fermyon Cloud
    Info(InfoCommand),
}

#[derive(Parser, Debug)]
pub struct ListCommand {
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct DeleteCommand {
    /// Name of Spin app
    pub app: String,
    #[clap(flatten)]
    common: CommonArgs,
}

#[derive(Parser, Debug)]
pub struct InfoCommand {
    /// Name of Spin app
    pub app: String,
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

impl AppsCommand {
    pub async fn run(self) -> Result<()> {
        match self {
            AppsCommand::List(cmd) => cmd.run().await,
            AppsCommand::Delete(cmd) => cmd.run().await,
            AppsCommand::Info(cmd) => cmd.run().await,
        }
    }
}

impl ListCommand {
    pub async fn run(self) -> Result<()> {
        let client = create_cloud_client(self.common.deployment_env_id.as_deref()).await?;
        let mut app_list_page = client.list_apps(DEFAULT_APPLIST_PAGE_SIZE, None).await?;
        if app_list_page.total_items <= 0 {
            eprintln!("No applications found");
        } else {
            print_app_list(&app_list_page);
            let mut page_index = 1;
            while !app_list_page.is_last_page {
                app_list_page = client
                    .list_apps(DEFAULT_APPLIST_PAGE_SIZE, Some(page_index))
                    .await?;
                print_app_list(&app_list_page);
                page_index += 1;
            }
        }
        Ok(())
    }
}

impl DeleteCommand {
    pub async fn run(self) -> Result<()> {
        let (client, app_id) =
            client_and_app_id(self.common.deployment_env_id.as_deref(), &self.app).await?;
        client
            .remove_app(app_id.to_string())
            .await
            .with_context(|| format!("Problem deleting app named {}", &self.app))?;
        println!("Deleted app \"{}\" successfully.", &self.app);
        Ok(())
    }
}

impl InfoCommand {
    pub async fn run(self) -> Result<()> {
        let (client, app_id) =
            client_and_app_id(self.common.deployment_env_id.as_deref(), &self.app).await?;
        let app = client
            .get_app(app_id.to_string())
            .await
            .with_context(|| format!("Error: could not get details about {}", &self.app))?;

        println!("Name: {}", &app.name);
        if let Some(description) = &app.description {
            if !description.is_empty() {
                println!("Description: {}", description);
            }
        }
        match app.domain {
            Some(val) => {
                match val.validation_status {
                    DomainValidationStatus::InProgress => {
                        if let Some(url) = app.channels[0].domain.as_ref() {
                            println!("URL: https://{}", url);
                            println!("Validation for {} is in progress", val.name)
                        }
                    }
                    DomainValidationStatus::Ready => {
                        println!("URL: https://{}", val.name);
                    }
                };
            }
            None => {
                if let Some(url) = app.channels[0].domain.as_ref() {
                    println!("URL: https://{}", url);
                }
            }
        };
        Ok(())
    }
}

fn print_app_list(page: &AppItemPage) {
    for app in &page.items {
        println!("{}", app.name);
    }
}
