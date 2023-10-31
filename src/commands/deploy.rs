use anyhow::{anyhow, bail, ensure, Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use cloud::{
    client::{Client as CloudClient, ConnectionConfig},
    CloudClientExt, CloudClientInterface,
};
use cloud_openapi::models::{
    ChannelRevisionSelectionStrategy as CloudChannelRevisionSelectionStrategy, Database,
    ResourceLabel,
};
use oci_distribution::{token_cache, Reference, RegistryOperation};
use spin_common::arg_parser::parse_kv;
use spin_http::{app_info::AppInfo, routes::RoutePattern};
use spin_locked_app::locked;
use tokio::fs;
use tracing::instrument;

use std::{
    collections::HashSet,
    io::{self, Write},
    path::{Path, PathBuf},
};
use url::Url;
use uuid::Uuid;

use crate::{
    commands::variables::{get_variables, set_variables},
    random_name::RandomNameGenerator,
    spin,
};

use crate::{
    commands::login::{LoginCommand, LoginConnection},
    opts::*,
};

use super::sqlite::database_has_link;

const SPIN_DEPLOY_CHANNEL_NAME: &str = "spin-deploy";
const SPIN_DEFAULT_KV_STORE: &str = "default";

/// Package and upload an application to the Fermyon Cloud.
#[derive(Parser, Debug)]
#[clap(about = "Package and upload an application to the Fermyon Cloud")]
pub struct DeployCommand {
    /// The application to deploy. This may be a manifest (spin.toml) file, a
    /// directory containing a spin.toml file, or a remote registry reference.
    /// If omitted, it defaults to "spin.toml".
    #[clap(
        name = APPLICATION_OPT,
        short = 'f',
        long = "from",
        group = "source",
    )]
    pub app_source: Option<String>,

    /// The application to deploy. This is the same as `--from` but forces the
    /// application to be interpreted as a file or directory path.
    #[clap(
        hide = true,
        name = APP_MANIFEST_FILE_OPT,
        long = "from-file",
        alias = "file",
        group = "source",
    )]
    pub file_source: Option<PathBuf>,

    /// The application to deploy. This is the same as `--from` but forces the
    /// application to be interpreted as an OCI registry reference.
    #[clap(
        hide = true,
        name = FROM_REGISTRY_OPT,
        long = "from-registry",
        group = "source",
    )]
    pub registry_source: Option<String>,

    /// For local apps, specifies to perform `spin build` before deploying the application.
    ///
    /// This is ignored on remote applications, as they are already built.
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

    fn resolve_app_source(&self) -> AppSource {
        match (&self.app_source, &self.file_source, &self.registry_source) {
            (None, None, None) => self.default_manifest_or_none(),
            (Some(source), None, None) => Self::infer_source(source),
            (None, Some(file), None) => Self::infer_file_source(file.to_owned()),
            (None, None, Some(reference)) => AppSource::OciRegistry(reference.to_owned()),
            _ => AppSource::unresolvable("More than one application source was specified"),
        }
    }

    fn default_manifest_or_none(&self) -> AppSource {
        let default_manifest = PathBuf::from(DEFAULT_MANIFEST_FILE);
        if default_manifest.exists() {
            AppSource::File(default_manifest)
        } else {
            AppSource::None
        }
    }

    fn infer_source(source: &str) -> AppSource {
        let path = PathBuf::from(source);
        if path.exists() {
            Self::infer_file_source(path)
        } else if spin_oci::is_probably_oci_reference(source) {
            AppSource::OciRegistry(source.to_owned())
        } else {
            AppSource::Unresolvable(format!("File or directory '{source}' not found. If you meant to load from a registry, use the `--from-registry` option."))
        }
    }

    fn infer_file_source(path: impl Into<PathBuf>) -> AppSource {
        match spin_common::paths::resolve_manifest_file_path(path.into()) {
            Ok(file) => AppSource::File(file),
            Err(e) => AppSource::Unresolvable(e.to_string()),
        }
    }

    async fn deploy_cloud(self, login_connection: LoginConnection) -> Result<()> {
        let connection_config = ConnectionConfig {
            url: login_connection.url.to_string(),
            insecure: login_connection.danger_accept_invalid_certs,
            token: login_connection.token.clone(),
        };

        let client = CloudClient::new(connection_config.clone());

        let dir = tempfile::tempdir()?;

        let application = self.load_cloud_app(dir.path()).await?;

        validate_cloud_app(&application)?;
        self.validate_deployment_environment(&application, &client)
            .await?;

        let digest = self
            .push_oci(application.clone(), connection_config.clone())
            .await?;

        let name = sanitize_app_name(application.name()?);
        let storage_id = format!("oci://{}", name);
        let version = sanitize_app_version(application.version()?);

        println!("Deploying...");

        // Create or update app
        let channel_id = match client.get_app_id(&name).await? {
            Some(app_id) => {
                let labels = application.sqlite_databases();
                if !labels.is_empty()
                    && create_and_link_databases_for_existing_app(&client, &name, app_id, labels)
                        .await?
                        .is_none()
                {
                    // User canceled terminal interaction
                    return Ok(());
                }
                client
                    .add_revision(storage_id.clone(), version.clone())
                    .await?;
                let existing_channel_id = client
                    .get_channel_id(app_id, SPIN_DEPLOY_CHANNEL_NAME)
                    .await?;
                let active_revision_id = client.get_revision_id(app_id, &version).await?;
                client
                    .patch_channel(
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
                    client
                        .add_key_value_pair(app_id, SPIN_DEFAULT_KV_STORE.to_string(), kv.0, kv.1)
                        .await
                        .context("Problem creating key/value")?;
                }

                set_variables(&client, app_id, &self.variables).await?;

                existing_channel_id
            }
            None => {
                let labels = application.sqlite_databases();
                let databases_to_link =
                    match create_databases_for_new_app(&client, &name, labels).await? {
                        Some(dbs) => dbs,
                        None => return Ok(()), // User canceled terminal interaction
                    };

                let app_id = client
                    .add_app(&name, &storage_id)
                    .await
                    .context("Unable to create app")?;

                // Now that the app has been created, we can link databases to it.
                link_databases(&client, name, app_id, databases_to_link).await?;

                client
                    .add_revision(storage_id.clone(), version.clone())
                    .await?;

                let active_revision_id = client.get_revision_id(app_id, &version).await?;

                let channel_id = client
                    .add_channel(
                        app_id,
                        String::from(SPIN_DEPLOY_CHANNEL_NAME),
                        CloudChannelRevisionSelectionStrategy::UseSpecifiedRevision,
                        None,
                        Some(active_revision_id),
                    )
                    .await
                    .context("Problem creating a channel")?;

                for kv in self.key_values {
                    client
                        .add_key_value_pair(app_id, SPIN_DEFAULT_KV_STORE.to_string(), kv.0, kv.1)
                        .await
                        .context("Problem creating key/value")?;
                }

                set_variables(&client, app_id, &self.variables).await?;

                channel_id
            }
        };

        let channel = client
            .get_channel_by_id(&channel_id.to_string())
            .await
            .context("Problem getting channel by id")?;
        let app_base_url = build_app_base_url(&channel.domain, &login_connection.url)?;
        let (http_base, http_routes) = application.http_routes();
        if !http_routes.is_empty() {
            wait_for_ready(
                &app_base_url,
                &digest.unwrap_or_default(),
                self.readiness_timeout_secs,
                Destination::Cloud(connection_config.clone().url),
            )
            .await;
            let base = http_base.unwrap_or_else(|| "/".to_owned());
            print_available_routes(&app_base_url, &base, &http_routes);
        } else {
            println!("Application is running at {}", channel.domain);
        }

        Ok(())
    }

    async fn load_cloud_app(&self, working_dir: &Path) -> Result<DeployableApp, anyhow::Error> {
        let app_source = self.resolve_app_source();

        let locked_app = match &app_source {
            AppSource::File(app_file) => {
                spin_loader::from_file(
                    &app_file,
                    spin_loader::FilesMountStrategy::Copy(working_dir.to_owned()),
                )
                .await?
            }
            AppSource::OciRegistry(reference) => {
                let mut oci_client = spin_oci::Client::new(false, None)
                    .await
                    .context("cannot create registry client")?;

                spin_oci::OciLoader::new(working_dir)
                    .load_app(&mut oci_client, reference)
                    .await?
            }
            AppSource::None => {
                anyhow::bail!("Default file '{DEFAULT_MANIFEST_FILE}' not found.");
            }
            AppSource::Unresolvable(err) => {
                anyhow::bail!("{err}");
            }
        };

        let unsupported_triggers = locked_app
            .triggers
            .iter()
            .filter(|t| t.trigger_type != "http")
            .map(|t| format!("'{}'", t.trigger_type))
            .collect::<Vec<_>>();
        if !unsupported_triggers.is_empty() {
            bail!(
                "Non-HTTP triggers are not supported - app uses {}",
                unsupported_triggers.join(", ")
            );
        }

        let locked_app = ensure_http_base_set(locked_app);

        Ok(DeployableApp(locked_app))
    }

    async fn validate_deployment_environment(
        &self,
        app: &DeployableApp,
        client: &CloudClient,
    ) -> Result<()> {
        let required_variables = app
            .0
            .variables
            .iter()
            .filter(|(_, v)| v.default.is_none())
            .map(|(k, _)| k)
            .collect::<HashSet<_>>();
        if !required_variables.is_empty() {
            self.ensure_variables_present(&required_variables, client, app.name()?)
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
        let extant_variables = match client.get_app_id(name).await {
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

    async fn push_oci(
        &self,
        application: DeployableApp,
        connection_config: ConnectionConfig,
    ) -> Result<Option<String>> {
        let mut client = spin_oci::Client::new(connection_config.insecure, None).await?;

        let cloud_url =
            Url::parse(connection_config.url.as_str()).context("Unable to parse cloud URL")?;
        let cloud_host = cloud_url
            .host_str()
            .context("Unable to derive host from cloud URL")?;
        let cloud_registry_host = format!("registry.{cloud_host}");

        let reference = format!(
            "{}/{}:{}",
            cloud_registry_host,
            &sanitize_app_name(application.name()?),
            &sanitize_app_version(application.version()?)
        );

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
            &oci_ref.tag().unwrap_or(application.version()?)
        );
        let digest = client.push_locked(application.0, reference).await?;

        Ok(digest)
    }

    async fn run_spin_build(&self) -> Result<()> {
        self.resolve_app_source().build().await
    }
}

// Spin now allows HTTP apps to omit the base path, but Cloud
// doesn't yet like this. This works around that by defaulting
// base if not set. (We don't check trigger type because by the
// time this is called we know it's HTTP.)
fn ensure_http_base_set(mut locked_app: locked::LockedApp) -> locked::LockedApp {
    if let Some(trigger) = locked_app
        .metadata
        .entry("trigger")
        .or_insert_with(|| serde_json::Value::Object(Default::default()))
        .as_object_mut()
    {
        trigger.entry("base").or_insert_with(|| "/".into());
    }

    locked_app
}

#[derive(Debug, PartialEq, Eq)]
enum AppSource {
    None,
    File(PathBuf),
    OciRegistry(String),
    Unresolvable(String),
}

impl AppSource {
    fn unresolvable(message: impl Into<String>) -> Self {
        Self::Unresolvable(message.into())
    }

    async fn build(&self) -> anyhow::Result<()> {
        match self {
            Self::File(manifest_path) => {
                let spin_bin = spin::bin_path()?;

                let result = tokio::process::Command::new(spin_bin)
                    .args(["build", "-f"])
                    .arg(manifest_path)
                    .status()
                    .await
                    .context("Failed to execute `spin build` command")?;

                if result.success() {
                    Ok(())
                } else {
                    Err(anyhow!("Build failed: deployment cancelled"))
                }
            }
            _ => Ok(()),
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

fn validate_cloud_app(app: &DeployableApp) -> Result<()> {
    check_safe_app_name(app.name()?)?;
    ensure!(!app.components().is_empty(), "No components in spin.toml!");
    for component in app.components() {
        if let Some(invalid_store) = component
            .key_value_stores()
            .iter()
            .find(|store| *store != SPIN_DEFAULT_KV_STORE)
        {
            bail!("Invalid store {invalid_store:?} for component {:?}. Cloud currently supports only the 'default' store.", component.id());
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
        .any(|d| database_has_link(d, &resource_label.label, resource_label.app_name.as_deref()))
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
        1 => prompt_link_to_new_database(
            name,
            label,
            databases
                .iter()
                .map(|d| d.name.as_str())
                .collect::<HashSet<_>>(),
        ),
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

fn prompt_link_to_new_database(
    name: &str,
    label: &str,
    existing_names: HashSet<&str>,
) -> Result<DatabaseSelection> {
    let generator = RandomNameGenerator::new();
    let default_name = generator
        .generate_unique(existing_names, 20)
        .context("could not generate unique database name")?;

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

// Loops through an app's manifest and creates databases.
// Returns a list of database and label pairs that should be
// linked to the app once it is created.
// Returns None if the user canceled terminal interaction
async fn create_databases_for_new_app(
    client: &CloudClient,
    name: &str,
    labels: HashSet<String>,
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
    labels: HashSet<String>,
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
                    client
                        .create_database_link(&db, resource_label)
                        .await
                        .with_context(|| {
                            format!(r#"Could not link database "{}" to app "{}""#, db, app_name,)
                        })?;
                }
            }
        }
    }
    Ok(Some(()))
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
        client
            .create_database_link(&database, resource_label)
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

#[derive(Clone)]
struct DeployableApp(locked::LockedApp);

struct DeployableComponent(locked::LockedComponent);

impl DeployableApp {
    fn name(&self) -> anyhow::Result<&str> {
        self.0
            .metadata
            .get("name")
            .ok_or(anyhow!("Application has no name"))?
            .as_str()
            .ok_or(anyhow!("Application name is not a string"))
    }

    fn version(&self) -> anyhow::Result<&str> {
        self.0
            .metadata
            .get("version")
            .ok_or(anyhow!("Application has no version"))?
            .as_str()
            .ok_or(anyhow!("Application version is not a string"))
    }

    fn components(&self) -> Vec<DeployableComponent> {
        self.0
            .components
            .iter()
            .map(|c| DeployableComponent(c.clone()))
            .collect()
    }

    fn sqlite_databases(&self) -> HashSet<String> {
        self.components()
            .iter()
            .flat_map(|c| c.sqlite_databases())
            .collect()
    }

    fn http_routes(&self) -> (Option<String>, Vec<HttpRoute>) {
        let base = self
            .0
            .metadata
            .get("trigger")
            .and_then(|v| v.get("base"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let routes = self
            .0
            .triggers
            .iter()
            .filter_map(|t| self.http_route(t))
            .collect();
        (base, routes)
    }

    fn http_route(&self, trigger: &locked::LockedTrigger) -> Option<HttpRoute> {
        if &trigger.trigger_type != "http" {
            return None;
        }

        let Some(id) = trigger
            .trigger_config
            .get("component")
            .and_then(|v| v.as_str())
        else {
            return None;
        };

        let description = self.component_description(id).map(|s| s.to_owned());
        let route = trigger.trigger_config.get("route").and_then(|v| v.as_str());
        route.map(|r| HttpRoute {
            id: id.to_owned(),
            description,
            route_pattern: r.to_owned(),
        })
    }

    fn component_description(&self, id: &str) -> Option<&str> {
        self.0
            .components
            .iter()
            .find(|c| c.id == id)
            .and_then(|c| c.metadata.get("description").and_then(|v| v.as_str()))
    }
}

#[derive(Debug)]
struct HttpRoute {
    id: String,
    description: Option<String>,
    route_pattern: String,
}

impl DeployableComponent {
    fn id(&self) -> &str {
        &self.0.id
    }

    fn key_value_stores(&self) -> Vec<String> {
        self.metadata_vec_string("key_value_stores")
    }

    fn sqlite_databases(&self) -> Vec<String> {
        self.metadata_vec_string("databases")
    }

    fn metadata_vec_string(&self, key: &str) -> Vec<String> {
        let Some(raw) = self.0.metadata.get(key) else {
            return vec![];
        };
        let Some(arr) = raw.as_array() else {
            return vec![];
        };
        arr.iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.to_owned())
            .collect()
    }
}

fn build_app_base_url(app_domain: &str, cloud_url: &Url) -> Result<Url> {
    // HACK: We assume that the scheme (https vs http) of apps will match that of Cloud...
    let scheme = cloud_url.scheme();
    Url::parse(&format!("{scheme}://{app_domain}/")).with_context(|| {
        format!("Could not construct app base URL for {app_domain:?} (Cloud URL: {cloud_url:?})",)
    })
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

fn print_available_routes(app_base_url: &Url, base: &str, routes: &[HttpRoute]) {
    if routes.is_empty() {
        return;
    }

    // Strip any trailing slash from base URL
    let app_base_url = app_base_url.to_string();
    let route_prefix = app_base_url.strip_suffix('/').unwrap_or(&app_base_url);

    // Ensure base starts with a /
    let base = if !base.starts_with('/') {
        format!("/{base}")
    } else {
        base.to_owned()
    };

    println!("Available Routes:");
    for component in routes {
        let route = RoutePattern::from(&base, &component.route_pattern);
        println!("  {}: {}{}", component.id, route_prefix, route);
        if let Some(description) = &component.description {
            println!("    {}", description);
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

    fn deploy_cmd_for_test_file(filename: &str) -> DeployCommand {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(filename);
        DeployCommand {
            app_source: None,
            file_source: Some(path),
            registry_source: None,
            build: false,
            readiness_timeout_secs: 60,
            deployment_env_id: None,
            key_values: vec![],
            variables: vec![],
        }
    }

    fn get_trigger_base(mut app: DeployableApp) -> String {
        let serde_json::map::Entry::Occupied(trigger) = app.0.metadata.entry("trigger") else {
            panic!("Expected trigger metadata but entry was vacant");
        };
        let base = trigger
            .get()
            .as_object()
            .unwrap()
            .get("base")
            .expect("Manifest should have had a base but didn't");
        base.as_str()
            .expect("HTTP base should have been a string but wasn't")
            .to_owned()
    }

    #[tokio::test]
    async fn if_http_base_is_set_then_it_is_respected() {
        let temp_dir = tempfile::tempdir().unwrap();

        let cmd = deploy_cmd_for_test_file("based_v1.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/base", base);

        let cmd = deploy_cmd_for_test_file("based_v2.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/base", base);
    }

    #[tokio::test]
    async fn if_http_base_is_not_set_then_it_is_inserted() {
        let temp_dir = tempfile::tempdir().unwrap();

        let cmd = deploy_cmd_for_test_file("unbased_v1.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/", base);

        let cmd = deploy_cmd_for_test_file("unbased_v2.toml");
        let app = cmd.load_cloud_app(temp_dir.path()).await.unwrap();
        let base = get_trigger_base(app);
        assert_eq!("/", base);
    }
}
