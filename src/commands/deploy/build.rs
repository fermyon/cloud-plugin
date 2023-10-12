use anyhow::{anyhow, Context};

use super::app_source::AppSource;
use crate::spin;

impl AppSource {
    pub(super) async fn build(&self) -> anyhow::Result<()> {
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
