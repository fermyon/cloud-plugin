use anyhow::{anyhow, bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use cloud::client::{Client as CloudClient, ConnectionConfig};
use cloud_openapi::models::{
    ChannelRevisionSelectionStrategy as CloudChannelRevisionSelectionStrategy, Database,
    ResourceLabel,
};
use oci_distribution::{token_cache, Reference, RegistryOperation};
use rand::Rng;
use semver::BuildMetadata;
use spin_common::{arg_parser::parse_kv, sloth};
use spin_http::{app_info::AppInfo, routes::RoutePattern};
use spin_loader::local::config::{self, RawAppManifest};
use spin_manifest::{ApplicationTrigger, HttpTriggerConfiguration, TriggerConfig};
use tokio::fs;
use tracing::instrument;

use std::{
    collections::HashSet,
    io::{self, Write},
    path::PathBuf,
};
use url::Url;
use uuid::Uuid;

use crate::{
    commands::{
        get_app_id_cloud,
        variables::{get_variables, set_variables},
    },
    spin,
};

use crate::{
    commands::login::{LoginCommand, LoginConnection},
    opts::*,
    parse_buildinfo,
};

use super::sqlite::database_has_link;

const SPIN_DEPLOY_CHANNEL_NAME: &str = "spin-deploy";
const SPIN_DEFAULT_KV_STORE: &str = "default";
const SPIN_DEFAULT_DATABASE: &str = "default";

/// Package and upload an application to the Fermyon Cloud.
#[derive(Parser, Debug)]
#[clap(about = "Package and upload an application to the Fermyon Cloud")]
pub struct DeployCommand {
    /// The application to deploy. This may be a manifest (spin.toml) file, or a
    /// directory containing a spin.toml file.
    /// If omitted, it defaults to "spin.toml".
    #[clap(
        name = APP_MANIFEST_FILE_OPT,
        short = 'f',
        long = "from",
        alias = "file",
        default_value = DEFAULT_MANIFEST_FILE
    )]
    pub app_source: PathBuf,

    /// Disable attaching buildinfo
    #[clap(
        long = "no-buildinfo",
        conflicts_with = BUILDINFO_OPT,
        env = "SPIN_DEPLOY_NO_BUILDINFO"
    )]
    pub no_buildinfo: bool,

    /// Build metadata to append to the oci tag
    #[clap(
        name = BUILDINFO_OPT,
        long = "buildinfo",
        parse(try_from_str = parse_buildinfo),
    )]
    pub buildinfo: Option<BuildMetadata>,

    /// Specifies to perform `spin build` before deploying the application.
    #[clap(long, takes_value = false, env = "SPIN_ALWAYS_BUILD")]
    pub build: bool,

    /// How long in seconds to wait for a deployed HTTP application to become
    /// ready. The default is 60 seconds. Set it to 0 to skip waiting
    /// for readiness.
    #[clap(long = "readiness-timeout", default_value = "60")]
    pub readiness_timeout_secs: u16,

    /// Deploy to the Fermyon instance saved under the specified name.
    /// If omitted, Spin deploys to the default unnamed instance.
    #[clap(
        name = "environment-name",
        long = "environment-name",
        env = DEPLOYMENT_ENV_NAME_ENV
    )]
    pub deployment_env_id: Option<String>,

    /// Set a key/value pair (key=value) in the deployed application's
    /// default store. Any existing value will be overwritten.
    /// Can be used multiple times.
    #[clap(long = "key-value", parse(try_from_str = parse_kv))]
    pub key_values: Vec<(String, String)>,

    /// Set a variable (variable=value) in the deployed application.
    /// Any existing value will be overwritten.
    /// Can be used multiple times.
    #[clap(long = "variable", parse(try_from_str = parse_kv))]
    pub variables: Vec<(String, String)>,
}

impl DeployCommand {
    pub async fn run(self) -> Result<()> {
        if self.build {
            self.run_spin_build().await?;
        }

        let login_connection = login_connection(self.deployment_env_id.as_deref()).await?;

        const DEVELOPER_CLOUD_FAQ: &str = "https://developer.fermyon.com/cloud/faq";

        self.deploy_cloud(login_connection)
            .await
            .map_err(|e| anyhow!("{:?}\n\nLearn more at {}", e, DEVELOPER_CLOUD_FAQ))
    }

    fn app(&self) -> anyhow::Result<PathBuf> {
        spin_common::paths::resolve_manifest_file_path(&self.app_source)
    }

    async fn deploy_cloud(self, login_connection: LoginConnection) -> Result<()> {
        let connection_config = ConnectionConfig {
            url: login_connection.url.to_string(),
            insecure: login_connection.danger_accept_invalid_certs,
            token: login_connection.token.clone(),
        };

        let client = CloudClient::new(connection_config.clone());

        let cfg_any = spin_loader::local::raw_manifest_from_file(&self.app()?).await?;
        let cfg = cfg_any.into_v1();

        validate_cloud_app(&cfg)?;
        self.validate_deployment_environment(&cfg, &client).await?;

        match cfg.info.trigger {
            ApplicationTrigger::Http(_) => {}
            ApplicationTrigger::Redis(_) => bail!("Redis triggers are not supported"),
            ApplicationTrigger::External(_) => bail!("External triggers are not supported"),
        }

        let buildinfo = if !self.no_buildinfo {
            match &self.buildinfo {
                Some(i) => Some(i.clone()),
                None => Some(random_buildinfo()),
            }
        } else {
            None
        };

        let app_file = spin_common::paths::resolve_manifest_file_path(&self.app_source)?;
        let dir = tempfile::tempdir()?;
        let application = spin_loader::local::from_file(&app_file, Some(dir.path())).await?;

        // TODO: can remove once spin_oci inlines small files by default
        std::env::set_var("SPIN_OCI_SKIP_INLINED_FILES", "true");

        let digest = self
            .push_oci(
                application.clone(),
                buildinfo.clone(),
                connection_config.clone(),
            )
            .await?;

        let name = sanitize_app_name(&application.info.name);
        let storage_id = format!("oci://{}", name);
        let version = sanitize_app_version(
            &(application.info.version
                + "-"
                + &buildinfo.clone().context("Cannot parse build info")?),
        );

        println!("Deploying...");

        // Create or update app
        let channel_id = match get_app_id_cloud(&client, &name).await? {
            Some(app_id) => {
                let labels = sqlite_labels_used(&cfg);
                if !labels.is_empty()
                    && create_and_link_databases_for_existing_app(&client, &name, app_id, labels)
                        .await?
                        .is_none()
                {
                    // User canceled terminal interaction
                    return Ok(());
                }
                CloudClient::add_revision(&client, storage_id.clone(), version.clone()).await?;
                let existing_channel_id = self
                    .get_channel_id_cloud(&client, SPIN_DEPLOY_CHANNEL_NAME.to_string(), app_id)
                    .await?;
                let active_revision_id = self
                    .get_revision_id_cloud(&client, version.clone(), app_id)
                    .await?;
                CloudClient::patch_channel(
                    &client,
                    existing_channel_id,
                    None,
                    Some(CloudChannelRevisionSelectionStrategy::UseSpecifiedRevision),
                    None,
                    Some(active_revision_id),
                    None,
                )
                .await
                .context("Problem patching a channel")?;

                for kv in self.key_values {
                    CloudClient::add_key_value_pair(
                        &client,
                        app_id,
                        SPIN_DEFAULT_KV_STORE.to_string(),
                        kv.0,
                        kv.1,
                    )
                    .await
                    .context("Problem creating key/value")?;
                }

                set_variables(&client, app_id, &self.variables).await?;

                existing_channel_id
            }
            None => {
                let labels = sqlite_labels_used(&cfg);
                let databases_to_link =
                    match create_databases_for_new_app(&client, &name, labels).await? {
                        Some(dbs) => dbs,
                        None => return Ok(()), // User canceled terminal interaction
                    };

                let app_id = CloudClient::add_app(&client, &name, &storage_id)
                    .await
                    .context("Unable to create app")?;

                // Now that the app has been created, we can link databases to it.
                link_databases(&client, name, app_id, databases_to_link).await?;

                CloudClient::add_revision(&client, storage_id.clone(), version.clone()).await?;

                let active_revision_id = self
                    .get_revision_id_cloud(&client, version.clone(), app_id)
                    .await?;

                let channel_id = CloudClient::add_channel(
                    &client,
                    app_id,
                    String::from(SPIN_DEPLOY_CHANNEL_NAME),
                    CloudChannelRevisionSelectionStrategy::UseSpecifiedRevision,
                    None,
                    Some(active_revision_id),
                )
                .await
                .context("Problem creating a channel")?;

                for kv in self.key_values {
                    CloudClient::add_key_value_pair(
                        &client,
                        app_id,
                        SPIN_DEFAULT_KV_STORE.to_string(),
                        kv.0,
                        kv.1,
                    )
                    .await
                    .context("Problem creating key/value")?;
                }

                set_variables(&client, app_id, &self.variables).await?;

                channel_id
            }
        };

        let channel = CloudClient::get_channel_by_id(&client, &channel_id.to_string())
            .await
            .context("Problem getting channel by id")?;
        let app_base_url = build_app_base_url(&channel.domain, &login_connection.url)?;
        if let Ok(http_config) = HttpTriggerConfiguration::try_from(cfg.info.trigger.clone()) {
            wait_for_ready(
                &app_base_url,
                &digest.unwrap_or_default(),
                self.readiness_timeout_secs,
                Destination::Cloud(connection_config.clone().url),
            )
            .await;
            print_available_routes(&app_base_url, &http_config.base, &cfg);
        } else {
            println!("Application is running at {}", channel.domain);
        }

        Ok(())
    }

    async fn validate_deployment_environment(
        &self,
        app: &RawAppManifest,
        client: &CloudClient,
    ) -> Result<()> {
        let required_variables = app
            .variables
            .iter()
            .filter(|(_, v)| v.required)
            .map(|(k, _)| k)
            .collect::<HashSet<_>>();
        if !required_variables.is_empty() {
            self.ensure_variables_present(&required_variables, client, &app.info.name)
                .await?;
        }
        Ok(())
    }

    async fn ensure_variables_present(
        &self,
        required_variables: &HashSet<&String>,
        client: &CloudClient,
        name: &str,
    ) -> Result<()> {
        // Are all required variables satisifed by variables passed in this command?
        let provided_variables = self.variables.iter().map(|(k, _)| k).collect();
        let unprovided_variables = required_variables
            .difference(&provided_variables)
            .copied()
            .collect::<HashSet<_>>();
        if unprovided_variables.is_empty() {
            return Ok(());
        }

        // Are all remaining required variables satisfied by variables already in the cloud?
        let extant_variables = match self.try_get_app_id_cloud(client, name.to_string()).await {
            Ok(Some(app_id)) => match get_variables(client, app_id).await {
                Ok(variables) => variables,
                Err(_) => {
                    // Don't block deployment for being unable to check the variables.
                    eprintln!("Unable to confirm variables {unprovided_variables:?} are defined. Check your app after deployment.");
                    return Ok(());
                }
            },
            Ok(None) => vec![],
            Err(_) => {
                // Don't block deployment for being unable to check the variables.
                eprintln!("Unable to confirm variables {unprovided_variables:?} are defined. Check your app after deployment.");
                return Ok(());
            }
        };
        let extant_variables = extant_variables.iter().map(|v| &v.key).collect();
        let unprovided_variables = unprovided_variables
            .difference(&extant_variables)
            .map(|v| v.as_str())
            .collect::<Vec<_>>();
        if unprovided_variables.is_empty() {
            return Ok(());
        }

        let list_text = unprovided_variables.join(", ");
        Err(anyhow!("The application requires values for the following variable(s) which have not been set: {list_text}. Use the --variable flag to provide values."))
    }

    async fn try_get_app_id_cloud(
        &self,
        cloud_client: &CloudClient,
        name: String,
    ) -> Result<Option<Uuid>> {
        let apps_vm = CloudClient::list_apps(cloud_client, DEFAULT_APPLIST_PAGE_SIZE, None).await?;
        let app = apps_vm.items.iter().find(|&x| x.name == name.clone());
        match app {
            Some(a) => Ok(Some(a.id)),
            None => Ok(None),
        }
    }

    async fn get_revision_id_cloud(
        &self,
        cloud_client: &CloudClient,
        version: String,
        app_id: Uuid,
    ) -> Result<Uuid> {
        let mut revisions = cloud_client.list_revisions().await?;

        loop {
            if let Some(revision) = revisions
                .items
                .iter()
                .find(|&x| x.revision_number == version && x.app_id == app_id)
            {
                return Ok(revision.id);
            }

            if revisions.is_last_page {
                break;
            }

            revisions = cloud_client.list_revisions_next(&revisions).await?;
        }

        Err(anyhow!(
            "No revision with version {} and app id {}",
            version,
            app_id
        ))
    }

    async fn get_channel_id_cloud(
        &self,
        cloud_client: &CloudClient,
        name: String,
        app_id: Uuid,
    ) -> Result<Uuid> {
        let mut channels_vm = cloud_client.list_channels().await?;

        loop {
            if let Some(channel) = channels_vm
                .items
                .iter()
                .find(|&x| x.app_id == app_id && x.name == name.clone())
            {
                return Ok(channel.id);
            }

            if channels_vm.is_last_page {
                break;
            }

            channels_vm = cloud_client.list_channels_next(&channels_vm).await?;
        }

        Err(anyhow!(
            "No channel with app_id {} and name {}",
            app_id,
            name
        ))
    }

    async fn push_oci(
        &self,
        application: spin_manifest::Application,
        buildinfo: Option<BuildMetadata>,
        connection_config: ConnectionConfig,
    ) -> Result<Option<String>> {
        let mut client = spin_oci::Client::new(connection_config.insecure, None).await?;

        let cloud_url =
            Url::parse(connection_config.url.as_str()).context("Unable to parse cloud URL")?;
        let cloud_host = cloud_url
            .host_str()
            .context("Unable to derive host from cloud URL")?;
        let cloud_registry_host = format!("registry.{cloud_host}");

        let reference = match buildinfo {
            Some(buildinfo) => {
                format!(
                    "{}/{}:{}",
                    cloud_registry_host,
                    &sanitize_app_name(&application.info.name),
                    &sanitize_app_version(
                        &(application.info.version.to_owned() + "-" + &buildinfo),
                    )
                )
            }
            None => {
                format!(
                    "{}/{}",
                    cloud_registry_host,
                    &sanitize_app_name(&application.info.name)
                )
            }
        };

        let oci_ref = Reference::try_from(reference.as_ref())
            .context(format!("Could not parse reference '{reference}'"))?;

        client.insert_token(
            &oci_ref,
            RegistryOperation::Push,
            token_cache::RegistryTokenType::Bearer(token_cache::RegistryToken::Token {
                token: connection_config.token,
            }),
        );

        println!(
            "Uploading {} version {} to Fermyon Cloud...",
            &oci_ref.repository(),
            &oci_ref.tag().unwrap_or(&application.info.version)
        );
        let digest = client.push(&application, reference).await?;

        Ok(digest)
    }

    async fn run_spin_build(&self) -> Result<()> {
        let manifest_path = self.app()?;
        let spin_bin = spin::bin_path()?;

        let result = tokio::process::Command::new(spin_bin)
            .args(["build", "-f"])
            .arg(&manifest_path)
            .status()
            .await
            .context("Failed to execute `spin build` command")?;

        if result.success() {
            Ok(())
        } else {
            Err(anyhow!("Build failed: deployment cancelled"))
        }
    }
}

// SAFE_APP_NAME regex to only allow letters/numbers/underscores/dashes
lazy_static::lazy_static! {
    static ref SAFE_APP_NAME: regex::Regex = regex::Regex::new("^[-_\\p{L}\\p{N}]+$").expect("Invalid name regex");
}

// TODO: logic here inherited from bindle restrictions; it would be friendlier to users
// to be less stringent and do the necessary sanitization on the backend, rather than
// presenting this error at deploy time.
fn check_safe_app_name(name: &str) -> Result<()> {
    if SAFE_APP_NAME.is_match(name) {
        Ok(())
    } else {
        Err(anyhow!("App name '{}' contains characters that are not allowed. It may contain only letters, numbers, dashes and underscores", name))
    }
}

// Sanitize app name to conform to Docker repo name conventions
// From https://docs.docker.com/engine/reference/commandline/tag/#extended-description:
// The path consists of slash-separated components. Each component may contain lowercase letters, digits and separators.
// A separator is defined as a period, one or two underscores, or one or more hyphens. A component may not start or end with a separator.
fn sanitize_app_name(name: &str) -> String {
    name.to_ascii_lowercase()
        .replace(' ', "")
        .trim_start_matches(|c: char| c == '.' || c == '_' || c == '-')
        .trim_end_matches(|c: char| c == '.' || c == '_' || c == '-')
        .to_string()
}

// Sanitize app version to conform to Docker tag conventions
// From https://docs.docker.com/engine/reference/commandline/tag
// A tag name must be valid ASCII and may contain lowercase and uppercase letters, digits, underscores, periods and hyphens.
// A tag name may not start with a period or a hyphen and may contain a maximum of 128 characters.
fn sanitize_app_version(tag: &str) -> String {
    let mut sanitized = tag
        .trim()
        .trim_start_matches(|c: char| c == '.' || c == '-');

    if sanitized.len() > 128 {
        (sanitized, _) = sanitized.split_at(128);
    }
    sanitized.replace(' ', "")
}

fn validate_cloud_app(app: &RawAppManifest) -> Result<()> {
    check_safe_app_name(&app.info.name)?;
    ensure!(!app.components.is_empty(), "No components in spin.toml!");
    for component in &app.components {
        if let Some(invalid_store) = component
            .wasm
            .key_value_stores
            .iter()
            .flatten()
            .find(|store| *store != SPIN_DEFAULT_KV_STORE)
        {
            bail!("Invalid store {invalid_store:?} for component {:?}. Cloud currently supports only the 'default' store.", component.id);
        }

        if let Some(invalid_db) = component
            .wasm
            .sqlite_databases
            .iter()
            .flatten()
            .find(|db| *db != SPIN_DEFAULT_DATABASE)
        {
            bail!("Invalid database {invalid_db:?} for component {:?}. Cloud currently supports only the 'default' SQLite databases.", component.id);
        }
    }
    Ok(())
}

/// A user's selection of a database to link to a label
enum DatabaseSelection {
    Existing(String),
    New(String),
    Cancelled,
}

/// Whether a database has already been linked or not
enum ExistingAppDatabaseSelection {
    NotYetLinked(DatabaseSelection),
    AlreadyLinked,
}

async fn get_database_selection_for_existing_app(
    name: &str,
    client: &CloudClient,
    resource_label: &ResourceLabel,
) -> Result<ExistingAppDatabaseSelection> {
    let databases = client.get_databases(None).await?;
    if databases
        .iter()
        .any(|d| database_has_link(d, resource_label))
    {
        return Ok(ExistingAppDatabaseSelection::AlreadyLinked);
    }
    let selection = prompt_database_selection(name, &resource_label.label, databases)?;
    Ok(ExistingAppDatabaseSelection::NotYetLinked(selection))
}

async fn get_database_selection_for_new_app(
    name: &str,
    client: &CloudClient,
    label: &str,
) -> Result<DatabaseSelection> {
    let databases = client.get_databases(None).await?;
    prompt_database_selection(name, label, databases)
}

fn prompt_database_selection(
    name: &str,
    label: &str,
    databases: Vec<Database>,
) -> Result<DatabaseSelection> {
    let prompt = format!(
        r#"App "{name}" accesses a database labeled "{label}"
Would you like to link an existing database or create a new database?"#
    );
    let existing_opt = "Use an existing database and link app to it";
    let create_opt = "Create a new database and link the app to it";
    let opts = vec![existing_opt, create_opt];
    let index = match dialoguer::Select::new()
        .with_prompt(prompt)
        .items(&opts)
        .default(1)
        .interact_opt()?
    {
        Some(i) => i,
        None => return Ok(DatabaseSelection::Cancelled),
    };
    match index {
        0 => prompt_for_existing_database(
            name,
            label,
            databases.into_iter().map(|d| d.name).collect::<Vec<_>>(),
        ),
        1 => prompt_link_to_new_database(name, label),
        _ => bail!("Choose unavailable option"),
    }
}

fn prompt_for_existing_database(
    name: &str,
    label: &str,
    mut database_names: Vec<String>,
) -> Result<DatabaseSelection> {
    let prompt =
        format!(r#"Which database would you like to link to {name} using the label "{label}""#);
    let index = match dialoguer::Select::new()
        .with_prompt(prompt)
        .items(&database_names)
        .default(0)
        .interact_opt()?
    {
        Some(i) => i,
        None => return Ok(DatabaseSelection::Cancelled),
    };
    Ok(DatabaseSelection::Existing(database_names.remove(index)))
}

fn prompt_link_to_new_database(name: &str, label: &str) -> Result<DatabaseSelection> {
    // TODO: use random name generator
    let default_name = format!("{name}-{label}");
    let prompt = format!(
        r#"What would you like to name your database?
Note: This name is used when managing your database at the account level. The app "{name}" will refer to this database by the label "{label}".
Other apps can use different labels to refer to the same database."#
    );
    let name = dialoguer::Input::new()
        .with_prompt(prompt)
        .default(default_name)
        .interact_text()?;
    Ok(DatabaseSelection::New(name))
}

fn sqlite_labels_used(cfg: &config::RawAppManifestImpl<TriggerConfig>) -> Vec<String> {
    cfg.components
        .iter()
        .cloned()
        .filter_map(|c| c.wasm.sqlite_databases)
        .flatten()
        .collect()
}

// Loops through an app's manifest and creates databases.
// Returns a list of database and label pairs that should be
// linked to the app once it is created.
// Returns None if the user canceled terminal interaction
async fn create_databases_for_new_app(
    client: &CloudClient,
    name: &str,
    labels: Vec<String>,
) -> anyhow::Result<Option<Vec<(String, String)>>> {
    let mut databases_to_link = Vec::new();
    for label in labels {
        let db = match get_database_selection_for_new_app(name, client, &label).await? {
            DatabaseSelection::Existing(db) => db,
            DatabaseSelection::New(db) => {
                client.create_database(db.clone(), None).await?;
                db
            }
            // User canceled terminal interaction
            DatabaseSelection::Cancelled => return Ok(None),
        };
        databases_to_link.push((db, label));
    }
    Ok(Some(databases_to_link))
}

// Loops through an updated app's manifest and creates and links any newly referenced databases.
// Returns None if the user canceled terminal interaction
async fn create_and_link_databases_for_existing_app(
    client: &CloudClient,
    app_name: &str,
    app_id: Uuid,
    labels: Vec<String>,
) -> anyhow::Result<Option<()>> {
    for label in labels {
        let resource_label = ResourceLabel {
            app_id,
            label,
            app_name: Some(app_name.to_string()),
        };
        if let ExistingAppDatabaseSelection::NotYetLinked(selection) =
            get_database_selection_for_existing_app(app_name, client, &resource_label).await?
        {
            match selection {
                // User canceled terminal interaction
                DatabaseSelection::Cancelled => return Ok(None),
                DatabaseSelection::New(db) => {
                    client.create_database(db, Some(resource_label)).await?;
                }
                DatabaseSelection::Existing(db) => {
                    CloudClient::create_database_link(client, &db, resource_label)
                        .await
                        .with_context(|| {
                            format!(r#"Could not link database "{}" to app "{}""#, db, app_name,)
                        })?;
                }
            }
        }
    }
    Ok(None)
}

async fn link_databases(
    client: &CloudClient,
    app_name: String,
    app_id: Uuid,
    database_labels: Vec<(String, String)>,
) -> anyhow::Result<()> {
    for (database, label) in database_labels {
        let resource_label = ResourceLabel {
            label,
            app_id,
            app_name: Some(app_name.clone()),
        };
        CloudClient::create_database_link(client, &database, resource_label)
            .await
            .with_context(|| {
                format!(
                    r#"Failed to link database "{}" to app "{}""#,
                    database, app_name
                )
            })?;
    }
    Ok(())
}

fn random_buildinfo() -> BuildMetadata {
    let random_bytes: [u8; 4] = rand::thread_rng().gen();
    let random_hex: String = random_bytes.iter().map(|b| format!("{:x}", b)).collect();
    BuildMetadata::new(&format!("r{random_hex}")).unwrap()
}

fn build_app_base_url(app_domain: &str, cloud_url: &Url) -> Result<Url> {
    // HACK: We assume that the scheme (https vs http) of apps will match that of Cloud...
    let scheme = cloud_url.scheme();
    Url::parse(&format!("{scheme}://{app_domain}/")).with_context(|| {
        format!("Could not construct app base URL for {app_domain:?} (Cloud URL: {cloud_url:?})",)
    })
}

async fn check_healthz(base_url: &Url) -> Result<()> {
    let healthz_url = base_url.join("healthz")?;
    reqwest::get(healthz_url)
        .await?
        .error_for_status()
        .with_context(|| format!("Server {} is unhealthy", base_url))?;
    Ok(())
}

const READINESS_POLL_INTERVAL_SECS: u64 = 2;

enum Destination {
    Cloud(String),
}

async fn wait_for_ready(
    app_base_url: &Url,
    app_version: &str,
    readiness_timeout_secs: u16,
    destination: Destination,
) {
    if readiness_timeout_secs == 0 {
        return;
    }

    let app_info_url = app_base_url
        .join(spin_http::WELL_KNOWN_PREFIX.trim_start_matches('/'))
        .unwrap()
        .join("info")
        .unwrap()
        .to_string();

    let start = std::time::Instant::now();
    let readiness_timeout = std::time::Duration::from_secs(u64::from(readiness_timeout_secs));
    let poll_interval = tokio::time::Duration::from_secs(READINESS_POLL_INTERVAL_SECS);

    print!("Waiting for application to become ready");
    let _ = std::io::stdout().flush();
    loop {
        match is_ready(&app_info_url, app_version).await {
            Err(err) => {
                println!("... readiness check failed: {err:?}");
                return;
            }
            Ok(true) => {
                println!("... ready");
                return;
            }
            Ok(false) => {}
        }

        print!(".");
        let _ = std::io::stdout().flush();

        if start.elapsed() >= readiness_timeout {
            println!();
            println!("Application deployed, but Spin could not establish readiness");
            match destination {
                Destination::Cloud(url) => {
                    println!(
                        "Check the Fermyon Cloud dashboard to see the application status: {url}"
                    );
                }
            }
            return;
        }
        tokio::time::sleep(poll_interval).await;
    }
}

#[instrument(level = "debug")]
async fn is_ready(app_info_url: &str, expected_version: &str) -> Result<bool> {
    // If the request fails, we assume the app isn't ready
    let resp = match reqwest::get(app_info_url).await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!("Readiness check failed: {err:?}");
            return Ok(false);
        }
    };
    // If the response status isn't success, the app isn't ready
    if !resp.status().is_success() {
        tracing::debug!("App not ready: {}", resp.status());
        return Ok(false);
    }
    // If the app was previously deployed then it will have an outdated
    // version, in which case the app isn't ready
    if let Ok(app_info) = resp.json::<AppInfo>().await {
        let active_version = app_info.oci_image_digest;
        if active_version.as_deref() != Some(expected_version) {
            tracing::debug!("Active version {active_version:?} != expected {expected_version:?}");
            return Ok(false);
        }
    }
    Ok(true)
}

fn print_available_routes(
    app_base_url: &Url,
    base: &str,
    cfg: &spin_loader::local::config::RawAppManifest,
) {
    if cfg.components.is_empty() {
        return;
    }

    // Strip any trailing slash from base URL
    let app_base_url = app_base_url.to_string();
    let route_prefix = app_base_url.strip_suffix('/').unwrap_or(&app_base_url);

    println!("Available Routes:");
    for component in &cfg.components {
        if let TriggerConfig::Http(http_cfg) = &component.trigger {
            let route = RoutePattern::from(base, &http_cfg.route);
            println!("  {}: {}{}", component.id, route_prefix, route);
            if let Some(description) = &component.description {
                println!("    {}", description);
            }
        }
    }
}

// Check if the token has expired.
// If the expiration is None, assume the token has not expired
fn has_expired(login_connection: &LoginConnection) -> Result<bool> {
    match &login_connection.expiration {
        Some(expiration) => match DateTime::parse_from_rfc3339(expiration) {
            Ok(time) => Ok(Utc::now() > time),
            Err(err) => Err(anyhow!(
                "Failed to parse token expiration time '{}'. Error: {}",
                expiration,
                err
            )),
        },
        None => Ok(false),
    }
}

pub async fn login_connection(deployment_env_id: Option<&str>) -> Result<LoginConnection> {
    let path = config_file_path(deployment_env_id)?;

    // log in if config.json does not exist or cannot be read
    let data = match fs::read_to_string(path.clone()).await {
        Ok(d) => d,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match deployment_env_id {
                Some(name) => {
                    // TODO: allow auto redirect to login preserving the name
                    eprintln!("You have no instance saved as '{}'", name);
                    eprintln!("Run `spin login --environment-name {}` to log in", name);
                    std::process::exit(1);
                }
                None => {
                    // log in, then read config
                    // TODO: propagate deployment id (or bail if nondefault?)
                    LoginCommand::parse_from(vec!["login"]).run().await?;
                    fs::read_to_string(path.clone()).await?
                }
            }
        }
        Err(e) => {
            bail!("Could not log in: {}", e);
        }
    };

    let mut login_connection: LoginConnection = serde_json::from_str(&data)?;
    let expired = match has_expired(&login_connection) {
        Ok(val) => val,
        Err(err) => {
            eprintln!("{}\n", err);
            eprintln!("Run `spin login` to log in again");
            std::process::exit(1);
        }
    };

    if expired {
        // if we have a refresh token available, let's try to refresh the token
        match login_connection.refresh_token {
            Some(refresh_token) => {
                // Only Cloud has support for refresh tokens
                let connection_config = ConnectionConfig {
                    url: login_connection.url.to_string(),
                    insecure: login_connection.danger_accept_invalid_certs,
                    token: login_connection.token.clone(),
                };
                let client = CloudClient::new(connection_config.clone());

                match client
                    .refresh_token(login_connection.token, refresh_token)
                    .await
                {
                    Ok(token_info) => {
                        login_connection.token = token_info.token;
                        login_connection.refresh_token = Some(token_info.refresh_token);
                        login_connection.expiration = Some(token_info.expiration);
                        // save new token info
                        let path = config_file_path(deployment_env_id)?;
                        std::fs::write(path, serde_json::to_string_pretty(&login_connection)?)?;
                    }
                    Err(e) => {
                        eprintln!("Failed to refresh token: {}", e);
                        match deployment_env_id {
                            Some(name) => {
                                eprintln!(
                                    "Run `spin login --environment-name {}` to log in again",
                                    name
                                );
                            }
                            None => {
                                eprintln!("Run `spin login` to log in again");
                            }
                        }
                        std::process::exit(1);
                    }
                }
            }
            None => {
                // session has expired and we have no way to refresh the token - log back in
                match deployment_env_id {
                    Some(name) => {
                        // TODO: allow auto redirect to login preserving the name
                        eprintln!("Your login to this environment has expired");
                        eprintln!(
                            "Run `spin login --environment-name {}` to log in again",
                            name
                        );
                        std::process::exit(1);
                    }
                    None => {
                        LoginCommand::parse_from(vec!["login"]).run().await?;
                        let new_data = fs::read_to_string(path.clone()).await.context(format!(
                            "Cannot find spin config at {}",
                            path.to_string_lossy()
                        ))?;
                        login_connection = serde_json::from_str(&new_data)?;
                    }
                }
            }
        }
    }

    let sloth_guard = sloth::warn_if_slothful(
        2500,
        format!("Checking status ({})\n", login_connection.url),
    );
    check_healthz(&login_connection.url).await?;
    // Server has responded - we don't want to keep the sloth timer running.
    drop(sloth_guard);

    Ok(login_connection)
}

// TODO: unify with login
pub fn config_file_path(deployment_env_id: Option<&str>) -> Result<PathBuf> {
    let root = dirs::config_dir()
        .context("Cannot find configuration directory")?
        .join("fermyon");

    let file_stem = match deployment_env_id {
        None => "config",
        Some(id) => id,
    };
    let file = format!("{}.json", file_stem);

    let path = root.join(file);

    Ok(path)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn accepts_only_valid_app_names() {
        check_safe_app_name("hello").expect("should have accepted 'hello'");
        check_safe_app_name("hello-world").expect("should have accepted 'hello-world'");
        check_safe_app_name("hell0_w0rld").expect("should have accepted 'hell0_w0rld'");

        let _ =
            check_safe_app_name("hello/world").expect_err("should not have accepted 'hello/world'");

        let _ =
            check_safe_app_name("hello world").expect_err("should not have accepted 'hello world'");
    }

    #[test]
    fn should_sanitize_app_name() {
        assert_eq!("hello-world", sanitize_app_name("hello-world"));
        assert_eq!("hello-world2000", sanitize_app_name("Hello-World2000"));
        assert_eq!("hello-world", sanitize_app_name(".-_hello-world_-"));
        assert_eq!("hello-world", sanitize_app_name(" hello -world "));
    }

    #[test]
    fn should_sanitize_app_version() {
        assert_eq!("v0.1.0", sanitize_app_version("v0.1.0"));
        assert_eq!("_v0.1.0", sanitize_app_version("_v0.1.0"));
        assert_eq!("v0.1.0_-", sanitize_app_version(".-v0.1.0_-"));
        assert_eq!("v0.1.0", sanitize_app_version(" v 0.1.0 "));
        assert_eq!(
            "v0.1.0+Hello-World",
            sanitize_app_version(" v 0.1.0+Hello-World ")
        );
        assert_eq!(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            sanitize_app_version("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855e3b")
        );
    }
}
