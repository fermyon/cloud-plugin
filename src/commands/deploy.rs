use anyhow::{anyhow, bail, ensure, Context, Result};
use clap::Parser;
use cloud::{
    client::{Client as CloudClient, ConnectionConfig},
    CloudClientExt, CloudClientInterface,
};
use cloud_openapi::models::ChannelRevisionSelectionStrategy as CloudChannelRevisionSelectionStrategy;
use oci_distribution::{token_cache, Reference, RegistryOperation};
use spin_common::arg_parser::parse_kv;
use spin_http::{app_info::AppInfo, routes::RoutePattern};
use spin_manifest::ApplicationTrigger;
use tracing::instrument;

use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
};
use url::Url;

mod app_source;
mod build;
mod cancellable;
mod database;
mod interaction;
mod login;

use app_source::AppSource;
use cancellable::Cancellable;
use database::{
    create_and_link_databases_for_existing_app, create_databases_for_new_app, link_databases,
};
use interaction::Interactor;
pub use login::login_connection;

use crate::commands::variables::get_variables;

use crate::opts::*;

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

const DEVELOPER_CLOUD_FAQ: &str = "https://developer.fermyon.com/cloud/faq";

impl DeployCommand {
    pub async fn run(self) -> Result<()> {
        let app_source = self.resolve_app_source();

        if self.build {
            app_source.build().await?;
        }

        let login_connection = login_connection(self.deployment_env_id.as_deref()).await?;

        let connection_config = ConnectionConfig {
            url: login_connection.url.to_string(),
            insecure: login_connection.danger_accept_invalid_certs,
            token: login_connection.token.clone(),
        };
        let client = CloudClient::new(connection_config.clone());

        let interactor = interaction::interactive();

        self.deploy_cloud(
            &interactor,
            &client,
            &login_connection.url,
            connection_config,
            &app_source,
        )
        .await
        .map_err(|e| anyhow!("{:?}\n\nLearn more at {}", e, DEVELOPER_CLOUD_FAQ))
    }

    async fn deploy_cloud(
        self,
        interactor: &impl Interactor,
        client: &impl CloudClientInterface,
        cloud_url: &Url,
        connection_config: ConnectionConfig,
        app_source: &AppSource,
    ) -> Result<()> {
        let dir = tempfile::tempdir()?;

        let application = app_source.load_cloud_app(dir.path()).await?;

        validate_cloud_app(&application)?;
        self.validate_deployment_environment(&application, client)
            .await?;

        // TODO: can remove once spin_oci inlines small files by default
        std::env::set_var("SPIN_OCI_SKIP_INLINED_FILES", "true");

        let digest = push_oci(&application, cloud_url, &connection_config).await?;

        let app_name = sanitize_app_name(application.name()?);
        let storage_id = format!("oci://{}", app_name);
        let version = sanitize_app_version(application.version()?);

        println!("Deploying...");

        // Create or update app
        let deployment = match client.get_app_id(&app_name).await? {
            Some(app_id) => {
                update_app_and_resources(
                    interactor,
                    &application,
                    client,
                    app_id,
                    &app_name,
                    &storage_id,
                    &version,
                )
                .await?
            }
            None => {
                create_app_and_resources(
                    interactor,
                    &application,
                    client,
                    &app_name,
                    &storage_id,
                    &version,
                )
                .await?
            }
        };

        let Cancellable::Accepted(deployment) = deployment else {
            return Ok(());
        };

        client
            .set_key_values(deployment.app_id, SPIN_DEFAULT_KV_STORE, &self.key_values)
            .await?;
        client
            .set_variables(deployment.app_id, &self.variables)
            .await?;

        let channel = client
            .get_channel_by_id(&deployment.channel_id)
            .await
            .context("Problem getting channel by id")?;
        let app_base_url = build_app_base_url(&channel.domain, cloud_url)?;
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

    async fn validate_deployment_environment(
        &self,
        app: &DeployableApp,
        client: &impl CloudClientInterface,
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
        client: &impl CloudClientInterface,
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
}

async fn push_oci(
    application: &DeployableApp,
    cloud_url: &Url,
    connection_config: &ConnectionConfig,
) -> Result<Option<String>> {
    let mut client = spin_oci::Client::new(connection_config.insecure, None).await?;

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
            token: connection_config.token.clone(),
        }),
    );

    println!(
        "Uploading {} version {} to Fermyon Cloud...",
        &oci_ref.repository(),
        &oci_ref.tag().unwrap_or(application.version()?)
    );
    let digest = client.push_locked(application.0.clone(), reference).await?;

    Ok(digest)
}

async fn create_app_and_resources(
    interactor: &impl Interactor,
    application: &DeployableApp,
    client: &impl CloudClientInterface,
    app_name: &str,
    storage_id: &str,
    version: &str,
) -> anyhow::Result<Cancellable<Deployment>> {
    let labels = application.sqlite_databases();
    let databases_to_link =
        match create_databases_for_new_app(interactor, client, app_name, labels).await? {
            Some(dbs) => dbs,
            None => return Ok(Cancellable::Cancelled),
        };

    let app_id = client
        .add_app(app_name, storage_id)
        .await
        .context("Unable to create app")?;

    link_databases(client, app_name, app_id, databases_to_link).await?;

    client.add_revision_ref(storage_id, version).await?;

    let active_revision_id = client.get_revision_id(app_id, version).await?;
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

    let cloud_app = Deployment::new(app_id, channel_id);
    Ok(Cancellable::Accepted(cloud_app))
}

async fn update_app_and_resources(
    interactor: &impl Interactor,
    application: &DeployableApp,
    client: &impl CloudClientInterface,
    app_id: uuid::Uuid,
    app_name: &str,
    storage_id: &str,
    version: &str,
) -> anyhow::Result<Cancellable<Deployment>> {
    let labels = application.sqlite_databases();
    if !labels.is_empty()
        && create_and_link_databases_for_existing_app(interactor, client, app_name, app_id, labels)
            .await?
            .is_none()
    {
        // User canceled terminal interaction
        return Ok(Cancellable::Cancelled);
    }

    client.add_revision_ref(storage_id, version).await?;

    let active_revision_id = client.get_revision_id(app_id, version).await?;
    let existing_channel_id = client
        .get_channel_id(app_id, SPIN_DEPLOY_CHANNEL_NAME)
        .await?;
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

    let cloud_app = Deployment::new(app_id, existing_channel_id);
    Ok(Cancellable::Accepted(cloud_app))
}

struct Deployment {
    pub app_id: uuid::Uuid,
    pub channel_id: String,
}

impl Deployment {
    pub fn new(app_id: uuid::Uuid, channel_id: uuid::Uuid) -> Self {
        Self {
            app_id,
            channel_id: channel_id.to_string(),
        }
    }
}

impl AppSource {
    async fn load_cloud_app(&self, working_dir: &Path) -> Result<DeployableApp, anyhow::Error> {
        match self {
            AppSource::File(app_file) => {
                let cfg_any = spin_loader::local::raw_manifest_from_file(&app_file).await?;
                let cfg = cfg_any.into_v1();

                match cfg.info.trigger {
                    ApplicationTrigger::Http(_) => {}
                    ApplicationTrigger::Redis(_) => bail!("Redis triggers are not supported"),
                    ApplicationTrigger::External(_) => bail!("External triggers are not supported"),
                }

                let app = spin_loader::from_file(app_file, Some(working_dir)).await?;
                let locked_app = spin_trigger::locked::build_locked_app(app, working_dir)?;

                Ok(DeployableApp(locked_app))
            }
            AppSource::OciRegistry(reference) => {
                let mut oci_client = spin_oci::Client::new(false, None)
                    .await
                    .context("cannot create registry client")?;

                let locked_app = spin_oci::OciLoader::new(working_dir)
                    .load_app(&mut oci_client, reference)
                    .await?;

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

                Ok(DeployableApp(locked_app))
            }
            AppSource::None => {
                anyhow::bail!("Default file '{DEFAULT_MANIFEST_FILE}' not found.");
            }
            AppSource::Unresolvable(err) => {
                anyhow::bail!("{err}");
            }
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

#[derive(Clone)]
struct DeployableApp(spin_app::locked::LockedApp);

struct DeployableComponent(spin_app::locked::LockedComponent);

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

    fn http_route(&self, trigger: &spin_app::locked::LockedTrigger) -> Option<HttpRoute> {
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

    println!("Available Routes:");
    for component in routes {
        let route = RoutePattern::from(base, &component.route_pattern);
        println!("  {}: {}{}", component.id, route_prefix, route);
        if let Some(description) = &component.description {
            println!("    {}", description);
        }
    }
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
