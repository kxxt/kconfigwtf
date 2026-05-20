use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use minijinja::{Environment, context};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::index::{ConfigValue, PackageIndex, normalize_config_name};

const MANIFEST_FILE_NAME: &str = "indexes.json";
const DATA_OUTPUT_DIR: &str = "data";
const CONFIG_OUTPUT_DIR: &str = "CONFIG_";
const MAX_ARCHITECTURES_PER_TAG: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteManifest {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
    pub indexes: Vec<String>,
    pub configs: Vec<String>,
}

#[derive(Debug, Clone)]
struct LoadedPackageIndex {
    url: String,
    index: PackageIndex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderRecord {
    distribution: String,
    package_name: String,
    version: String,
    architecture: String,
    value: String,
    source: Option<String>,
    config_url: String,
}

pub struct SiteGenerator {
    title: String,
}

impl SiteGenerator {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }

    pub fn generate(&self, data_dir: impl AsRef<Path>, output_dir: impl AsRef<Path>) -> Result<()> {
        let data_dir = data_dir.as_ref();
        let output_dir = output_dir.as_ref();
        fs::create_dir_all(output_dir)
            .with_context(|| format!("creating site output directory {}", output_dir.display()))?;

        let package_index_paths = find_package_indexes(data_dir)?;
        let loaded_indexes = load_package_indexes(data_dir, &package_index_paths)?;
        copy_data_dir(data_dir, &output_dir.join(DATA_OUTPUT_DIR))?;
        let manifest = build_manifest(&loaded_indexes);

        let mut env = Environment::new();
        env.add_template("index.html", include_str!("templates/index.html"))
            .context("registering index.html template")?;

        write_page(
            &env,
            output_dir.join("index.html"),
            PageRender {
                site_title: &self.title,
                page_title: &self.title,
                asset_prefix: "",
                manifest_file: MANIFEST_FILE_NAME,
                result_title: "Enter a config entry",
                result_count: "",
                table_body: r#"<tr><td colspan="5" class="empty">No lookup has been run yet.</td></tr>"#,
                config_viewer_hidden: true,
            },
        )?;

        for config in &manifest.configs {
            let config_name = normalize_config_name(config);
            let records = records_for_config(&config_name, &loaded_indexes, "../../");
            let page_dir = output_dir.join(CONFIG_OUTPUT_DIR).join(config);
            fs::create_dir_all(&page_dir).with_context(|| {
                format!("creating config page directory {}", page_dir.display())
            })?;
            write_page(
                &env,
                page_dir.join("index.html"),
                PageRender {
                    site_title: &self.title,
                    page_title: &format!("{config_name} - {}", self.title),
                    asset_prefix: "../../",
                    manifest_file: MANIFEST_FILE_NAME,
                    result_title: &config_name,
                    result_count: &format!(
                        "{} match{}",
                        records.len(),
                        if records.len() == 1 { "" } else { "es" }
                    ),
                    table_body: &render_results_table(&records),
                    config_viewer_hidden: true,
                },
            )?;
        }

        fs::write(output_dir.join("app.js"), include_str!("templates/app.js"))
            .with_context(|| format!("writing {}", output_dir.join("app.js").display()))?;
        fs::write(
            output_dir.join("styles.css"),
            include_str!("templates/styles.css"),
        )
        .with_context(|| format!("writing {}", output_dir.join("styles.css").display()))?;

        let manifest_json =
            serde_json::to_string_pretty(&manifest).context("serializing site manifest")?;
        fs::write(output_dir.join(MANIFEST_FILE_NAME), manifest_json).with_context(|| {
            format!("writing {}", output_dir.join(MANIFEST_FILE_NAME).display())
        })?;

        Ok(())
    }
}

struct PageRender<'a> {
    site_title: &'a str,
    page_title: &'a str,
    asset_prefix: &'a str,
    manifest_file: &'a str,
    result_title: &'a str,
    result_count: &'a str,
    table_body: &'a str,
    config_viewer_hidden: bool,
}

fn write_page(env: &Environment<'_>, path: PathBuf, page: PageRender<'_>) -> Result<()> {
    let html = env
        .get_template("index.html")
        .context("loading index.html template")?
        .render(context! {
            site_title => page.site_title,
            page_title => page.page_title,
            asset_prefix => page.asset_prefix,
            manifest_file => page.manifest_file,
            result_title => page.result_title,
            result_count => page.result_count,
            table_body => page.table_body,
            config_viewer_hidden => page.config_viewer_hidden,
        })
        .context("rendering index.html")?;

    fs::write(&path, html).with_context(|| format!("writing {}", path.display()))
}

pub fn find_package_indexes(data_dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    let data_dir = data_dir.as_ref();
    let mut indexes = Vec::new();

    for entry in WalkDir::new(data_dir) {
        let entry = entry.with_context(|| format!("walking {}", data_dir.display()))?;
        if !entry.file_type().is_file() || entry.file_name() != "index.json" {
            continue;
        }

        read_package_index(entry.path())?;
        indexes.push(entry.path().to_path_buf());
    }

    indexes.sort();
    Ok(indexes)
}

fn load_package_indexes(
    data_dir: &Path,
    package_indexes: &[PathBuf],
) -> Result<Vec<LoadedPackageIndex>> {
    package_indexes
        .iter()
        .map(|index_path| {
            let relative = index_path.strip_prefix(data_dir).with_context(|| {
                format!(
                    "package index {} is not under {}",
                    index_path.display(),
                    data_dir.display()
                )
            })?;
            Ok(LoadedPackageIndex {
                url: format!(
                    "{DATA_OUTPUT_DIR}/{}",
                    relative.to_string_lossy().replace('\\', "/")
                ),
                index: read_package_index(index_path)?,
            })
        })
        .collect()
}

fn build_manifest(package_indexes: &[LoadedPackageIndex]) -> SiteManifest {
    let mut configs = BTreeSet::new();
    for package_index in package_indexes {
        configs.extend(
            package_index
                .index
                .entries
                .keys()
                .map(|name| name.strip_prefix("CONFIG_").unwrap_or(name).to_string()),
        );
    }

    SiteManifest {
        schema_version: 1,
        generated_at: Utc::now(),
        indexes: package_indexes
            .iter()
            .map(|package_index| package_index.url.clone())
            .collect(),
        configs: configs.into_iter().collect(),
    }
}

fn records_for_config(
    config_name: &str,
    package_indexes: &[LoadedPackageIndex],
    asset_prefix: &str,
) -> Vec<RenderRecord> {
    let mut records = Vec::new();

    for package_index in package_indexes {
        let Some(occurrences) = package_index.index.entries.get(config_name) else {
            continue;
        };
        let occurrence_by_kernel = occurrences
            .iter()
            .map(|occurrence| (occurrence.kernel.as_str(), &occurrence.value))
            .collect::<BTreeMap<_, _>>();
        let index_base = package_index
            .url
            .rsplit_once('/')
            .map(|(base, _)| base)
            .unwrap_or("");

        for (kernel_id, kernel) in &package_index.index.kernels {
            let value = occurrence_by_kernel
                .get(kernel_id.as_str())
                .map(|value| value.as_display_value().to_string())
                .unwrap_or_else(|| ConfigValue::Missing.as_display_value().to_string());
            records.push(RenderRecord {
                distribution: package_index.index.distribution.to_string(),
                package_name: package_index.index.package_name.clone(),
                version: kernel.version.clone(),
                architecture: kernel.architecture.to_string(),
                value,
                source: kernel.source.clone(),
                config_url: format!("{asset_prefix}{index_base}/{}", kernel.config_path),
            });
        }
    }

    records.sort_by(|left, right| {
        (
            &left.distribution,
            &left.package_name,
            &left.version,
            &left.architecture,
        )
            .cmp(&(
                &right.distribution,
                &right.package_name,
                &right.version,
                &right.architecture,
            ))
    });
    records
}

fn render_results_table(records: &[RenderRecord]) -> String {
    if records.is_empty() {
        return r#"<tr><td colspan="5" class="empty">No indexed kernel config contains this entry.</td></tr>"#.to_string();
    }

    let mut distributions: BTreeMap<&str, BTreeMap<&str, BTreeMap<&str, Vec<&RenderRecord>>>> =
        BTreeMap::new();
    for record in records {
        distributions
            .entry(&record.distribution)
            .or_default()
            .entry(&record.package_name)
            .or_default()
            .entry(&record.value)
            .or_default()
            .push(record);
    }

    let mut html = String::new();
    for (distribution, packages) in distributions {
        let distribution_rowspan = packages.values().map(|values| values.len()).sum::<usize>();
        let mut wrote_distribution = false;

        for (package, value_groups) in packages {
            let package_rowspan = value_groups.len();
            let mut wrote_package = false;

            for (value, records) in value_groups {
                html.push_str("<tr>");
                if !wrote_distribution {
                    html.push_str(&format!(
                        r#"<td rowspan="{distribution_rowspan}" class="group-cell">{}</td>"#,
                        escape_html(distribution)
                    ));
                    wrote_distribution = true;
                }
                if !wrote_package {
                    html.push_str(&format!(
                        r#"<td rowspan="{package_rowspan}" class="group-cell package-cell">{}</td>"#,
                        escape_html(package)
                    ));
                    wrote_package = true;
                }
                html.push_str(&format!("<td>{}</td>", escape_html(value)));
                html.push_str("<td>");
                html.push_str(&render_version_tags(&records));
                html.push_str("</td>");
                html.push_str("<td>");
                html.push_str(&render_sources(&records));
                html.push_str("</td>");
                html.push_str("</tr>");
            }
        }
    }

    html
}

fn render_version_tags(records: &[&RenderRecord]) -> String {
    let mut versions: BTreeMap<&str, BTreeMap<&str, &RenderRecord>> = BTreeMap::new();
    for record in records {
        versions
            .entry(&record.version)
            .or_default()
            .entry(&record.architecture)
            .or_insert(record);
    }

    let mut html = r#"<div class="tag-list">"#.to_string();
    for (version, architectures) in versions {
        let title = format!(
            "{}: {}",
            version,
            architectures.keys().copied().collect::<Vec<_>>().join(", ")
        );
        html.push_str(&format!(
            r#"<div class="kernel-tag" title="{}"><span class="tag-version">{}</span>"#,
            escape_attr(&title),
            escape_html(version)
        ));

        if architectures.len() > MAX_ARCHITECTURES_PER_TAG {
            html.push_str(&format!(
                r#"<details class="arch-details"><summary>{} archs</summary><span class="tag-architectures">"#,
                architectures.len()
            ));
            for (architecture, record) in architectures {
                html.push_str(&render_arch_button(architecture, record));
            }
            html.push_str("</span></details>");
        } else {
            html.push_str(r#"<span class="tag-architectures">"#);
            for (architecture, record) in architectures {
                html.push_str(&render_arch_button(architecture, record));
            }
            html.push_str("</span>");
        }
        html.push_str("</div>");
    }
    html.push_str("</div>");
    html
}

fn render_arch_button(architecture: &str, record: &RenderRecord) -> String {
    let title = format!(
        "{} {} {}",
        record.package_name, record.version, record.architecture
    );
    format!(
        r#"<button type="button" class="arch-button" data-config-url="{}" data-config-title="{}">{}</button>"#,
        escape_attr(&record.config_url),
        escape_attr(&title),
        escape_html(architecture)
    )
}

fn render_sources(records: &[&RenderRecord]) -> String {
    let sources = records
        .iter()
        .filter_map(|record| record.source.as_deref())
        .collect::<BTreeSet<_>>();

    match sources.len() {
        0 => String::new(),
        1 => {
            let source = sources.into_iter().next().expect("source");
            format!(r#"<a href="{}">package</a>"#, escape_attr(source))
        }
        count => format!("{count} packages"),
    }
}

fn read_package_index(index_path: &Path) -> Result<PackageIndex> {
    let json = fs::read_to_string(index_path)
        .with_context(|| format!("reading {}", index_path.display()))?;
    serde_json::from_str::<PackageIndex>(&json)
        .with_context(|| format!("parsing package index {}", index_path.display()))
}

fn copy_data_dir(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)
        .with_context(|| format!("creating data output directory {}", destination.display()))?;

    for entry in WalkDir::new(source) {
        let entry = entry.with_context(|| format!("walking {}", source.display()))?;
        let relative = entry.path().strip_prefix(source).with_context(|| {
            format!(
                "data path {} is not under {}",
                entry.path().display(),
                source.display()
            )
        })?;
        if relative.as_os_str().is_empty() {
            continue;
        }

        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)
                .with_context(|| format!("creating directory {}", target.display()))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("creating directory {}", parent.display()))?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!("copying {} to {}", entry.path().display(), target.display())
            })?;
        }
    }

    Ok(())
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_attr(input: &str) -> String {
    escape_html(input).replace('"', "&quot;")
}

#[allow(dead_code)]
fn validate_config_page_name(config: &str) -> Result<()> {
    if config.is_empty() || config.contains('/') || config.contains('\\') {
        bail!("invalid config page name {config:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Architecture, Distribution, write_packages_to_data_dir};
    use crate::indexer::KernelConfigPackage;

    #[test]
    fn writes_static_site_files_from_data_directory() {
        let data = tempfile::tempdir().expect("data tempdir");
        let site = tempfile::tempdir().expect("site tempdir");
        write_packages_to_data_dir(
            [KernelConfigPackage {
                distribution: Distribution::Debian,
                package_name: "linux-image-amd64".to_string(),
                package_version: "6.1.0-1".to_string(),
                architecture: Architecture::Amd64,
                source: None,
                config_text: "CONFIG_BPF=y\nCONFIG_EXT4_FS=m\n".to_string(),
            }],
            data.path(),
        )
        .expect("write data");

        SiteGenerator::new("kconfigwtf")
            .generate(data.path(), site.path())
            .expect("generate site");

        assert!(site.path().join("index.html").exists());
        assert!(site.path().join("app.js").exists());
        assert!(site.path().join("styles.css").exists());
        assert!(site.path().join("indexes.json").exists());
        assert!(site.path().join("CONFIG_/BPF/index.html").exists());
        assert!(site.path().join("CONFIG_/EXT4_FS/index.html").exists());
        let manifest: SiteManifest = serde_json::from_str(
            &fs::read_to_string(site.path().join("indexes.json")).expect("read manifest"),
        )
        .expect("parse manifest");
        assert_eq!(manifest.configs, vec!["BPF", "EXT4_FS"]);
        assert!(
            site.path()
                .join("data/debian/linux-image-amd64/index.json")
                .exists()
        );
        assert!(
            site.path()
                .join("data/debian/linux-image-amd64/6.1.0-1/amd64/config")
                .exists()
        );

        let bpf_page =
            fs::read_to_string(site.path().join("CONFIG_/BPF/index.html")).expect("read page");
        assert!(bpf_page.contains("CONFIG_BPF"));
        assert!(bpf_page.contains(
            "data-config-url=\"../../data/debian/linux-image-amd64/6.1.0-1/amd64/config\""
        ));
    }
}
