use std::path::PathBuf;

use crate::opts::DEFAULT_MANIFEST_FILE;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum AppSource {
    None,
    File(PathBuf),
    OciRegistry(String),
    Unresolvable(String),
}

impl super::DeployCommand {
    pub(super) fn resolve_app_source(&self) -> AppSource {
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
}

impl AppSource {
    fn unresolvable(message: impl Into<String>) -> Self {
        Self::Unresolvable(message.into())
    }
}
