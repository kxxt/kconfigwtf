use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use minijinja::{Environment, context};

use crate::index::ConfigIndex;

const INDEX_FILE_NAME: &str = "index.json";

pub struct SiteGenerator {
    title: String,
}

impl SiteGenerator {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }

    pub fn generate(&self, index: &ConfigIndex, output_dir: impl AsRef<Path>) -> Result<()> {
        let output_dir = output_dir.as_ref();
        fs::create_dir_all(output_dir)
            .with_context(|| format!("creating site output directory {}", output_dir.display()))?;

        let mut env = Environment::new();
        env.add_template("index.html", include_str!("templates/index.html"))
            .context("registering index.html template")?;

        let html = env
            .get_template("index.html")
            .context("loading index.html template")?
            .render(context! {
                title => self.title.as_str(),
                index_file => INDEX_FILE_NAME,
            })
            .context("rendering index.html")?;

        fs::write(output_dir.join("index.html"), html)
            .with_context(|| format!("writing {}", output_dir.join("index.html").display()))?;
        fs::write(output_dir.join("app.js"), include_str!("templates/app.js"))
            .with_context(|| format!("writing {}", output_dir.join("app.js").display()))?;
        fs::write(
            output_dir.join("styles.css"),
            include_str!("templates/styles.css"),
        )
        .with_context(|| format!("writing {}", output_dir.join("styles.css").display()))?;

        let index_json = serde_json::to_string_pretty(index).context("serializing config index")?;
        fs::write(output_dir.join(INDEX_FILE_NAME), index_json)
            .with_context(|| format!("writing {}", output_dir.join(INDEX_FILE_NAME).display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{ConfigIndex, ConfigValue, KernelConfigRecord};

    #[test]
    fn writes_static_site_files() {
        let mut index = ConfigIndex::default();
        index.entries.insert(
            "CONFIG_BPF".to_string(),
            vec![KernelConfigRecord {
                distribution: "debian".to_string(),
                package_name: "linux-image-6.1.0-1-amd64".to_string(),
                package_version: "6.1.4-1".to_string(),
                architecture: "amd64".to_string(),
                value: ConfigValue::BuiltIn,
                source: None,
            }],
        );

        let temp = tempfile::tempdir().expect("tempdir");
        SiteGenerator::new("kconfigwtf")
            .generate(&index, temp.path())
            .expect("generate site");

        assert!(temp.path().join("index.html").exists());
        assert!(temp.path().join("app.js").exists());
        assert!(temp.path().join("styles.css").exists());
        assert!(temp.path().join("index.json").exists());
    }
}
