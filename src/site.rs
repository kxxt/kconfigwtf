use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use indicatif::{MultiProgress, ProgressBar, ProgressDrawTarget, ProgressStyle};
use minijinja::{Environment, context};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::index::{
    ConfigValue, PackageIndex, is_package_index_file_name, normalize_config_name,
    read_package_index,
};

const MANIFEST_FILE_NAME: &str = "indexes.json";
const CNAME_FILE_NAME: &str = "CNAME";
const SITE_CNAME: &str = "kconfigwtf.kxxt.dev";
const DATA_REPOSITORY_DIR: &str = "data";
const CONFIG_OUTPUT_DIR: &str = "CONFIG_";
const MAX_ARCHITECTURES_PER_TAG: usize = 4;
const DEFAULT_GITHUB_REPOSITORY: &str = "kxxt/kconfigwtf";
const GITHUB_RAW_HOST: &str = "https://raw.githubusercontent.com";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SiteManifest {
    pub schema_version: u32,
    pub generated_at: DateTime<Utc>,
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
    release: String,
    package_name: String,
    version: String,
    architecture: String,
    value: String,
    source: Option<String>,
    config_url: String,
}

pub struct SiteGenerator {
    title: String,
    parallelism: usize,
}

impl SiteGenerator {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            parallelism: default_parallelism(),
        }
    }

    pub fn with_parallelism(mut self, parallelism: usize) -> Result<Self> {
        if parallelism == 0 {
            bail!("site generator parallelism must be at least 1");
        }

        self.parallelism = parallelism;
        Ok(self)
    }

    pub fn generate(&self, data_dir: impl AsRef<Path>, output_dir: impl AsRef<Path>) -> Result<()> {
        let data_dir = data_dir.as_ref();
        let output_dir = output_dir.as_ref();
        let progress = SiteBuildProgress::new();
        fs::create_dir_all(output_dir)
            .with_context(|| format!("creating site output directory {}", output_dir.display()))?;
        let config_url_base = github_raw_data_url_base()?;

        let discovery_progress = progress.spinner("discovering package indexes")?;
        let package_index_paths =
            find_package_indexes_with_progress(data_dir, Some(&discovery_progress))?;
        discovery_progress.finish_with_message(format!(
            "discovered {} package indexes",
            package_index_paths.len()
        ));

        let load_progress = progress.bar(
            package_index_paths.len() as u64,
            format!("loading {} package indexes", package_index_paths.len()),
        )?;
        let loaded_indexes =
            load_package_indexes(data_dir, &package_index_paths, Some(&load_progress))?;
        load_progress.finish_with_message(format!(
            "loaded {} package indexes",
            package_index_paths.len()
        ));

        let manifest_progress = progress.spinner("building manifest")?;
        let manifest = build_manifest(&loaded_indexes);
        manifest_progress.finish_with_message(format!(
            "built manifest for {} configs",
            manifest.configs.len()
        ));

        let root_page_progress = progress.spinner("writing root page")?;
        write_page(
            output_dir.join("index.html"),
            PageRender {
                site_title: &self.title,
                page_title: &self.title,
                asset_prefix: "",
                manifest_file: MANIFEST_FILE_NAME,
                result_title: "Enter a config entry",
                result_count: "",
                lkddb_url: None,
                kernelconfig_url: None,
                table_body: r#"<tr><td colspan="6" class="empty">No lookup has been run yet.</td></tr>"#,
                config_viewer_hidden: true,
            },
        )?;
        root_page_progress.finish_with_message("wrote root page".to_string());

        write_config_pages(
            &manifest.configs,
            &loaded_indexes,
            output_dir,
            &self.title,
            &config_url_base,
            self.parallelism,
            &progress,
        )?;

        let assets_progress = progress.spinner("writing static assets")?;
        fs::write(output_dir.join("app.js"), include_str!("templates/app.js"))
            .with_context(|| format!("writing {}", output_dir.join("app.js").display()))?;
        fs::write(
            output_dir.join("styles.css"),
            include_str!("templates/styles.css"),
        )
        .with_context(|| format!("writing {}", output_dir.join("styles.css").display()))?;
        assets_progress.finish_with_message("wrote static assets".to_string());

        let manifest_json =
            serde_json::to_string_pretty(&manifest).context("serializing site manifest")?;
        let write_manifest_progress = progress.spinner("writing site manifest")?;
        fs::write(output_dir.join(MANIFEST_FILE_NAME), manifest_json).with_context(|| {
            format!("writing {}", output_dir.join(MANIFEST_FILE_NAME).display())
        })?;
        write_manifest_progress.finish_with_message("wrote site manifest".to_string());

        let cname_progress = progress.spinner("writing CNAME")?;
        fs::write(output_dir.join(CNAME_FILE_NAME), format!("{SITE_CNAME}\n"))
            .with_context(|| format!("writing {}", output_dir.join(CNAME_FILE_NAME).display()))?;
        cname_progress.finish_with_message("wrote CNAME".to_string());

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
    lkddb_url: Option<&'a str>,
    kernelconfig_url: Option<&'a str>,
    table_body: &'a str,
    config_viewer_hidden: bool,
}

fn write_page(path: PathBuf, page: PageRender<'_>) -> Result<()> {
    let env = page_environment()?;
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
            lkddb_url => page.lkddb_url,
            kernelconfig_url => page.kernelconfig_url,
            table_body => page.table_body,
            config_viewer_hidden => page.config_viewer_hidden,
        })
        .context("rendering index.html")?;

    fs::write(&path, html).with_context(|| format!("writing {}", path.display()))
}

fn page_environment() -> Result<Environment<'static>> {
    let mut env = Environment::new();
    env.add_template("index.html", include_str!("templates/index.html"))
        .context("registering index.html template")?;
    Ok(env)
}

fn default_parallelism() -> usize {
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
}

fn write_config_pages(
    configs: &[String],
    loaded_indexes: &[LoadedPackageIndex],
    output_dir: &Path,
    title: &str,
    config_url_base: &str,
    parallelism: usize,
    progress: &SiteBuildProgress,
) -> Result<()> {
    if configs.is_empty() {
        return Ok(());
    }

    let worker_count = parallelism.max(1).min(configs.len());
    let progress = ConfigPageProgress::new(progress, configs.len(), worker_count)?;
    if worker_count == 1 {
        let result = (|| -> Result<()> {
            let worker = progress.worker(0);
            for config in configs {
                worker.start(config);
                write_config_page(config, loaded_indexes, output_dir, title, config_url_base)?;
                worker.finish_item(config);
            }
            Ok(())
        })();
        progress.finish(result.is_ok());
        return result;
    }

    #[allow(clippy::redundant_closure_call)]
    let result = (|| -> Result<()> {
        let next = AtomicUsize::new(0);
        thread::scope(|scope| -> Result<()> {
            let mut handles = Vec::with_capacity(worker_count);
            for worker_index in 0..worker_count {
                let worker = progress.worker(worker_index);
                let next = &next;
                handles.push(scope.spawn(move || -> Result<()> {
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= configs.len() {
                            break;
                        }

                        let config = &configs[index];
                        worker.start(config);
                        write_config_page(
                            config,
                            loaded_indexes,
                            output_dir,
                            title,
                            config_url_base,
                        )?;
                        worker.finish_item(config);
                    }

                    worker.idle();
                    Ok(())
                }));
            }

            for handle in handles {
                let result = handle
                    .join()
                    .map_err(|_| anyhow!("site build worker panicked"))?;
                result?;
            }

            Ok(())
        })
    })();
    progress.finish(result.is_ok());
    result
}

fn write_config_page(
    config: &str,
    loaded_indexes: &[LoadedPackageIndex],
    output_dir: &Path,
    title: &str,
    config_url_base: &str,
) -> Result<()> {
    let config_name = normalize_config_name(config);
    let records = records_for_config(&config_name, loaded_indexes, config_url_base);
    let page_dir = output_dir.join(CONFIG_OUTPUT_DIR).join(config);
    fs::create_dir_all(&page_dir)
        .with_context(|| format!("creating config page directory {}", page_dir.display()))?;

    let page_title = format!("{config_name} - {title}");
    let result_count = format!(
        "{} match{}",
        records.len(),
        if records.len() == 1 { "" } else { "es" }
    );
    let table_body = render_results_table(&records);
    let lkddb_url = format!(
        "https://cateee.net/lkddb/web-lkddb/{}.html",
        config_name.strip_prefix("CONFIG_").unwrap_or(&config_name)
    );
    let kernelconfig_url = format!("https://www.kernelconfig.io/{config_name}");

    write_page(
        page_dir.join("index.html"),
        PageRender {
            site_title: title,
            page_title: &page_title,
            asset_prefix: "../../",
            manifest_file: MANIFEST_FILE_NAME,
            result_title: &config_name,
            result_count: &result_count,
            lkddb_url: Some(&lkddb_url),
            kernelconfig_url: Some(&kernelconfig_url),
            table_body: &table_body,
            config_viewer_hidden: true,
        },
    )
}

struct SiteBuildProgress {
    multi: Option<MultiProgress>,
}

impl SiteBuildProgress {
    fn new() -> Self {
        if !io::stderr().is_terminal() {
            return Self { multi: None };
        }

        Self {
            multi: Some(MultiProgress::with_draw_target(
                ProgressDrawTarget::stderr_with_hz(10),
            )),
        }
    }

    fn spinner(&self, message: impl Into<String>) -> Result<ProgressBar> {
        let spinner = self.add_progress_bar(ProgressBar::new_spinner());
        spinner.set_style(phase_spinner_style()?);
        spinner.enable_steady_tick(std::time::Duration::from_millis(100));
        spinner.set_message(message.into());
        Ok(spinner)
    }

    fn bar(&self, length: u64, message: impl Into<String>) -> Result<ProgressBar> {
        let bar = self.add_progress_bar(ProgressBar::new(length));
        bar.set_style(phase_bar_style()?);
        bar.set_message(message.into());
        Ok(bar)
    }

    fn add_progress_bar(&self, bar: ProgressBar) -> ProgressBar {
        match &self.multi {
            Some(multi) => multi.add(bar),
            None => ProgressBar::hidden(),
        }
    }
}

struct ConfigPageProgress {
    total: ProgressBar,
    workers: Vec<ProgressBar>,
}

impl ConfigPageProgress {
    fn new(
        progress: &SiteBuildProgress,
        total_configs: usize,
        worker_count: usize,
    ) -> Result<Self> {
        let total = progress.bar(
            total_configs as u64,
            format!("building {total_configs} config pages"),
        )?;

        let workers = if worker_count > 1 {
            let mut workers = Vec::with_capacity(worker_count);
            for index in 0..worker_count {
                let worker = progress.add_progress_bar(ProgressBar::new_spinner());
                worker.set_style(worker_progress_style()?);
                worker.enable_steady_tick(std::time::Duration::from_millis(100));
                worker.set_message(format!("worker {:02}: idle", index + 1));
                workers.push(worker);
            }
            workers
        } else {
            Vec::new()
        };

        Ok(Self { total, workers })
    }

    fn worker(&self, index: usize) -> SiteBuildWorkerProgress {
        SiteBuildWorkerProgress {
            total: self.total.clone(),
            worker: self.workers.get(index).cloned(),
            worker_index: index + 1,
        }
    }

    fn finish(&self, success: bool) {
        if success {
            self.total.finish_with_message(format!(
                "built {} config pages",
                self.total.length().unwrap_or_default()
            ));
        } else {
            self.total.abandon_with_message(format!(
                "site build stopped after {}/{} pages",
                self.total.position(),
                self.total.length().unwrap_or_default()
            ));
        }

        for worker in &self.workers {
            worker.finish_and_clear();
        }
    }
}

struct SiteBuildWorkerProgress {
    total: ProgressBar,
    worker: Option<ProgressBar>,
    worker_index: usize,
}

impl SiteBuildWorkerProgress {
    fn start(&self, config: &str) {
        if let Some(worker) = &self.worker {
            worker.set_message(format!("worker {:02}: {}", self.worker_index, config));
            worker.tick();
        }
    }

    fn finish_item(&self, config: &str) {
        self.total.inc(1);
        if let Some(worker) = &self.worker {
            worker.set_message(format!("worker {:02}: done {}", self.worker_index, config));
            worker.tick();
        }
    }

    fn idle(&self) {
        if let Some(worker) = &self.worker {
            worker.set_message(format!("worker {:02}: idle", self.worker_index));
        }
    }
}

fn phase_bar_style() -> Result<ProgressStyle> {
    ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
    )
    .map(|style| style.progress_chars("##-"))
    .context("building site progress bar style")
}

fn phase_spinner_style() -> Result<ProgressStyle> {
    ProgressStyle::with_template("{spinner:.green} [{elapsed_precise}] {msg}")
        .context("building site spinner progress style")
}

fn worker_progress_style() -> Result<ProgressStyle> {
    ProgressStyle::with_template("{spinner:.yellow} {msg}")
        .context("building worker site progress style")
}

pub fn find_package_indexes(data_dir: impl AsRef<Path>) -> Result<Vec<PathBuf>> {
    find_package_indexes_with_progress(data_dir, None)
}

fn find_package_indexes_with_progress(
    data_dir: impl AsRef<Path>,
    progress: Option<&ProgressBar>,
) -> Result<Vec<PathBuf>> {
    let data_dir = data_dir.as_ref();
    let mut indexes = Vec::new();

    for entry in WalkDir::new(data_dir) {
        let entry = entry.with_context(|| format!("walking {}", data_dir.display()))?;
        if let Some(progress) = progress {
            progress.tick();
        }
        let Some(name) = entry.file_name().to_str() else {
            continue;
        };
        if !entry.file_type().is_file() || !is_package_index_file_name(name) {
            continue;
        }

        indexes.push(entry.path().to_path_buf());
        if let Some(progress) = progress {
            progress.set_message(format!("discovering package indexes ({})", indexes.len()));
        }
    }

    indexes.sort();
    Ok(indexes)
}

fn load_package_indexes(
    data_dir: &Path,
    package_indexes: &[PathBuf],
    progress: Option<&ProgressBar>,
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
            let result = Ok(LoadedPackageIndex {
                url: format!(
                    "{DATA_REPOSITORY_DIR}/{}",
                    relative.to_string_lossy().replace('\\', "/")
                ),
                index: read_package_index(index_path)?,
            });
            if let Some(progress) = progress {
                progress.inc(1);
            }
            result
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
        configs: configs.into_iter().collect(),
    }
}

fn records_for_config(
    config_name: &str,
    package_indexes: &[LoadedPackageIndex],
    config_url_base: &str,
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
                release: kernel.release.clone(),
                package_name: package_index.index.package_name.clone(),
                version: kernel.version.clone(),
                architecture: kernel.architecture.to_string(),
                value,
                source: kernel.source.clone(),
                config_url: raw_github_url(
                    config_url_base,
                    &format!("{index_base}/{}", kernel.config_path),
                ),
            });
        }
    }

    records.sort_by(|left, right| {
        (
            &left.distribution,
            &left.release,
            &left.package_name,
            &left.version,
            &left.architecture,
        )
            .cmp(&(
                &right.distribution,
                &right.release,
                &right.package_name,
                &right.version,
                &right.architecture,
            ))
    });
    records
}

fn render_results_table(records: &[RenderRecord]) -> String {
    if records.is_empty() {
        return r#"<tr><td colspan="6" class="empty">No indexed kernel config contains this entry.</td></tr>"#.to_string();
    }

    type ValueGroups<'a> = BTreeMap<&'a str, Vec<&'a RenderRecord>>;
    type PackageGroups<'a> = BTreeMap<&'a str, ValueGroups<'a>>;
    type ReleaseGroups<'a> = BTreeMap<&'a str, PackageGroups<'a>>;

    let mut distributions: BTreeMap<&str, ReleaseGroups<'_>> = BTreeMap::new();
    for record in records {
        distributions
            .entry(&record.distribution)
            .or_default()
            .entry(&record.release)
            .or_default()
            .entry(&record.package_name)
            .or_default()
            .entry(&record.value)
            .or_default()
            .push(record);
    }

    let mut html = String::new();
    for (distribution, releases) in distributions {
        let distribution_rowspan = releases
            .values()
            .map(|packages| packages.values().map(|values| values.len()).sum::<usize>())
            .sum::<usize>();
        let mut wrote_distribution = false;

        for (release, packages) in releases {
            let release_rowspan = packages.values().map(|values| values.len()).sum::<usize>();
            let mut wrote_release = false;

            for (package, value_groups) in packages {
                let package_rowspan = value_groups.len();
                let mut wrote_package = false;

                for (value, records) in value_groups {
                    html.push_str("<tr>");
                    if !wrote_distribution {
                        html.push_str(&format!(
                            r#"<td rowspan="{distribution_rowspan}" class="group-cell group-cell-distribution"><span class="sticky-group-label">{}</span></td>"#,
                            escape_html(distribution)
                        ));
                        wrote_distribution = true;
                    }
                    if !wrote_release {
                        html.push_str(&format!(
                            r#"<td rowspan="{release_rowspan}" class="group-cell group-cell-release"><span class="sticky-group-label">{}</span></td>"#,
                            escape_html(release)
                        ));
                        wrote_release = true;
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

fn github_raw_data_url_base() -> Result<String> {
    let repository = std::env::var("KCONFIGWTF_GITHUB_REPOSITORY")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_GITHUB_REPOSITORY.to_string());
    let commit = match std::env::var("KCONFIGWTF_GIT_COMMIT")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        Some(commit) => commit,
        None => discover_git_commit()?,
    };
    let commit = commit.trim().to_string();

    if repository.split('/').count() != 2 {
        bail!("invalid GitHub repository {repository:?}");
    }
    if !is_full_git_commit_hash(&commit) {
        bail!(
            "unable to determine an exact git commit for raw GitHub URLs; set KCONFIGWTF_GIT_COMMIT or build from a git checkout"
        );
    }

    Ok(format!("{GITHUB_RAW_HOST}/{repository}/{commit}"))
}

fn discover_git_commit() -> Result<String> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "HEAD"])
        .output()
        .with_context(|| format!("running git rev-parse HEAD in {}", repo_root.display()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "running git rev-parse HEAD in {} failed: {}",
            repo_root.display(),
            stderr.trim()
        );
    }

    let commit = String::from_utf8(output.stdout).context("git rev-parse HEAD was not utf-8")?;
    Ok(commit.trim().to_string())
}

fn is_full_git_commit_hash(commit: &str) -> bool {
    commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn raw_github_url(base: &str, path: &str) -> String {
    format!("{base}/{}", encode_url_path(path))
}

fn encode_url_path(path: &str) -> String {
    path.split('/')
        .map(encode_url_segment)
        .collect::<Vec<_>>()
        .join("/")
}

fn encode_url_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if matches!(
            byte,
            b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
        ) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
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
                release: "trixie".to_string(),
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
        assert_eq!(
            fs::read_to_string(site.path().join("CNAME")).expect("read CNAME"),
            "kconfigwtf.kxxt.dev\n"
        );
        assert!(site.path().join("CONFIG_/BPF/index.html").exists());
        assert!(site.path().join("CONFIG_/EXT4_FS/index.html").exists());
        let manifest: SiteManifest = serde_json::from_str(
            &fs::read_to_string(site.path().join("indexes.json")).expect("read manifest"),
        )
        .expect("parse manifest");
        assert_eq!(manifest.configs, vec!["BPF", "EXT4_FS"]);
        assert!(
            !site
                .path()
                .join("data/debian/linux-image-amd64/index.json")
                .exists()
        );
        assert!(!site.path().join("data").exists());

        let bpf_page =
            fs::read_to_string(site.path().join("CONFIG_/BPF/index.html")).expect("read page");
        let commit = discover_git_commit().expect("discover git commit");
        assert!(bpf_page.contains("CONFIG_BPF"));
        assert!(bpf_page.contains("cateee.net"));
        assert!(bpf_page.contains("web-lkddb"));
        assert!(bpf_page.contains("kernelconfig.io"));
        assert!(bpf_page.contains(r#"target="_blank""#));
        assert!(bpf_page.contains(
            &format!(
                "data-config-url=\"https://raw.githubusercontent.com/kxxt/kconfigwtf/{commit}/data/debian/linux-image-amd64/6.1.0-1/amd64/config\""
            )
        ));
    }

    #[test]
    fn writes_static_site_files_with_parallel_workers() {
        let data = tempfile::tempdir().expect("data tempdir");
        let site = tempfile::tempdir().expect("site tempdir");
        write_packages_to_data_dir(
            [KernelConfigPackage {
                distribution: Distribution::Debian,
                release: "trixie".to_string(),
                package_name: "linux-image-amd64".to_string(),
                package_version: "6.1.0-1".to_string(),
                architecture: Architecture::Amd64,
                source: None,
                config_text: "CONFIG_BPF=y\nCONFIG_EXT4_FS=m\nCONFIG_INET=y\n".to_string(),
            }],
            data.path(),
        )
        .expect("write data");

        SiteGenerator::new("kconfigwtf")
            .with_parallelism(2)
            .expect("set parallelism")
            .generate(data.path(), site.path())
            .expect("generate site");

        assert!(site.path().join("CONFIG_/BPF/index.html").exists());
        assert!(site.path().join("CONFIG_/EXT4_FS/index.html").exists());
        assert!(site.path().join("CONFIG_/INET/index.html").exists());
    }

    #[test]
    fn rejects_zero_parallelism() {
        let error = match SiteGenerator::new("kconfigwtf").with_parallelism(0) {
            Ok(_) => panic!("parallelism should fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("at least 1"));
    }

    #[test]
    fn groups_results_by_distribution_release_and_package() {
        let html = render_results_table(&[
            RenderRecord {
                distribution: "debian".to_string(),
                release: "trixie".to_string(),
                package_name: "linux-image-amd64".to_string(),
                version: "6.1.0-1".to_string(),
                architecture: "amd64".to_string(),
                value: "y".to_string(),
                source: None,
                config_url: "https://raw.githubusercontent.com/kxxt/kconfigwtf/0123456789abcdef0123456789abcdef01234567/data/debian/linux-image-amd64/6.1.0-1/amd64/config".to_string(),
            },
            RenderRecord {
                distribution: "debian".to_string(),
                release: "trixie".to_string(),
                package_name: "linux-image-cloud-amd64".to_string(),
                version: "6.1.0-1".to_string(),
                architecture: "amd64".to_string(),
                value: "y".to_string(),
                source: None,
                config_url: "https://raw.githubusercontent.com/kxxt/kconfigwtf/0123456789abcdef0123456789abcdef01234567/data/debian/linux-image-cloud-amd64/6.1.0-1/amd64/config".to_string(),
            },
            RenderRecord {
                distribution: "debian".to_string(),
                release: "bookworm".to_string(),
                package_name: "linux-image-amd64".to_string(),
                version: "5.10.0-1".to_string(),
                architecture: "amd64".to_string(),
                value: "m".to_string(),
                source: None,
                config_url: "https://raw.githubusercontent.com/kxxt/kconfigwtf/0123456789abcdef0123456789abcdef01234567/data/debian/linux-image-amd64/5.10.0-1/amd64/config".to_string(),
            },
        ]);

        assert!(html.contains(
            r#"<td rowspan="3" class="group-cell group-cell-distribution"><span class="sticky-group-label">debian</span></td>"#
        ));
        assert!(html.contains(
            r#"<td rowspan="1" class="group-cell group-cell-release"><span class="sticky-group-label">bookworm</span></td>"#
        ));
        assert!(html.contains(
            r#"<td rowspan="2" class="group-cell group-cell-release"><span class="sticky-group-label">trixie</span></td>"#
        ));
        assert!(html.contains(
            r#"<td rowspan="1" class="group-cell package-cell">linux-image-cloud-amd64</td>"#
        ));
    }

    #[test]
    fn raw_github_urls_encode_placeholder_segments() {
        let url = raw_github_url(
            "https://raw.githubusercontent.com/kxxt/kconfigwtf/0123456789abcdef0123456789abcdef01234567",
            "data/debian/linux-image-<VERSION>-<ARCH>/6.1.0-1/amd64/config",
        );

        assert_eq!(
            url,
            "https://raw.githubusercontent.com/kxxt/kconfigwtf/0123456789abcdef0123456789abcdef01234567/data/debian/linux-image-%3CVERSION%3E-%3CARCH%3E/6.1.0-1/amd64/config"
        );
    }
}
