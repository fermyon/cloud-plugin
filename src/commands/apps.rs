use crate::commands::{apps_output::AppInfo, client_and_app_id, create_cloud_client, CommonArgs};
use anyhow::{Context, Result};
use clap::Parser;
use cloud::{CloudClientInterface, DEFAULT_APPLIST_PAGE_SIZE};
use cloud_openapi::models::{AppItem, ValidationStatus};

use super::apps_output::{print_app_info, print_app_list, OutputFormat};

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
    /// Desired output format
    #[clap(value_enum, long = "format", default_value = "plain")]
    format: OutputFormat,
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
    /// Desired output format
    #[clap(value_enum, long = "format", default_value = "plain")]
    format: OutputFormat,
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
        let mut apps: Vec<String> = vec![];
        let mut page_index = 1;
        for app in app_list_page.items {
            apps.push(app.name.clone());
        }
        while !app_list_page.is_last_page {
            app_list_page = client
                .list_apps(DEFAULT_APPLIST_PAGE_SIZE, Some(page_index))
                .await?;
            for app in app_list_page.items {
                apps.push(app.name.clone());
            }
            page_index += 1;
        }
        print_app_list(apps, self.format);
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

        let (current_domain, in_progress_domain) = domains_current_and_in_progress(&app);

        let info = AppInfo::new(
            app.name.clone(),
            app.description.clone(),
            current_domain.cloned(),
            in_progress_domain.is_none(),
        );

        print_app_info(info, self.format);
        Ok(())
    }
}

fn domains_current_and_in_progress(app: &AppItem) -> (Option<&String>, Option<&String>) {
    let auto_domain = &app.subdomain;
    match &app.domain {
        Some(val) => match val.validation_status {
            ValidationStatus::InProgress | ValidationStatus::Provisioning => {
                (Some(auto_domain), Some(&val.name))
            }
            ValidationStatus::Ready => (Some(&val.name), None),
            ValidationStatus::Error => (Some(auto_domain), None),
        },
        None => (Some(auto_domain), None),
    }
}
