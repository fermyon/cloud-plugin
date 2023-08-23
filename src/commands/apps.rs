use crate::commands::{client_and_app_id, create_cloud_client};
use crate::opts::*;
use anyhow::{Context, Result};
use clap::{Args, Parser};
use cloud_openapi::models::AppItemPage;

#[derive(Parser, Debug)]
#[clap(about = "Manage applications deployed to Fermyon Cloud")]
pub enum AppsCommand {
    /// List all the apps deployed in Fermyon Cloud
    List(ListCommand),
    /// Delete an app deployed in Fermyon Cloud
    Delete(DeleteCommand),
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
            AppsCommand::List(cmd) => {
                let client = create_cloud_client(cmd.common.deployment_env_id.as_deref()).await?;
                let mut app_list_page = client.list_apps(DEFAULT_APPLIST_PAGE_SIZE, None).await?;
                if app_list_page.total_items > 0 {
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
            }
            AppsCommand::Delete(cmd) => {
                let (client, app_id) =
                    client_and_app_id(cmd.common.deployment_env_id.as_deref(), &cmd.app).await?;
                client
                    .remove_app(app_id.to_string())
                    .await
                    .with_context(|| format!("Problem deleting app named {}", &cmd.app))?;
                println!("Deleted app \"{}\" successfully.", &cmd.app);
            }
        }
        Ok(())
    }
}

fn print_app_list(page: &AppItemPage) {
    for app in &page.items {
        println!("{}", app.name);
    }
}
