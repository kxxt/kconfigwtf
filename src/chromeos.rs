use std::fs::File;
use std::io::Cursor;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use ext4_view::Ext4;
use flate2::read::GzDecoder;
use futures_util::StreamExt;
use tempfile::NamedTempFile;

use crate::ikconfig::extract_ikconfig_from_image;
use crate::index::{Architecture, Distribution};
use crate::indexer::{KernelConfigIndexer, KernelConfigPackage, normalize_release_label};

const SECTOR_SIZE: u64 = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromeOsImageLocation {
    Url(String),
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeOsIndexerConfig {
    pub image: ChromeOsImageLocation,
    pub architecture: Architecture,
}

#[derive(Debug, Clone)]
pub struct ChromeOsIndexer {
    client: reqwest::Client,
    config: ChromeOsIndexerConfig,
}

impl ChromeOsIndexer {
    pub fn new(config: ChromeOsIndexerConfig) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    async fn prepare_image(&self) -> Result<PreparedImage> {
        match &self.config.image {
            ChromeOsImageLocation::Url(url) => {
                let download = download_to_temp(&self.client, url).await?;
                let source = url.clone();
                if is_zip_path(Path::new(url)) {
                    let image = unzip_image_to_temp(download.path())?;
                    Ok(PreparedImage {
                        _download: Some(download),
                        image,
                        source,
                    })
                } else {
                    Ok(PreparedImage {
                        _download: None,
                        image: PreparedLocalPath::Temp(download),
                        source,
                    })
                }
            }
            ChromeOsImageLocation::Path(path) => {
                let source = path.display().to_string();
                let image = if is_zip_path(path) {
                    unzip_image_to_temp(path)?
                } else {
                    PreparedLocalPath::Borrowed(path.clone())
                };
                Ok(PreparedImage {
                    _download: None,
                    image,
                    source,
                })
            }
        }
    }
}

#[async_trait]
impl KernelConfigIndexer for ChromeOsIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let prepared = self.prepare_image().await?;
        let extracted = extract_chromeos_config_from_image(prepared.image.path())
            .with_context(|| format!("extracting ChromeOS config from {}", prepared.source))?;

        Ok(vec![KernelConfigPackage {
            distribution: Distribution::ChromeOS,
            release: normalize_release_label(&extracted.platform_version),
            package_name: extracted.platform_version,
            package_version: extracted.kernel_version,
            architecture: self.config.architecture.clone(),
            source: Some(format!(
                "{}#{}:{}",
                prepared.source, extracted.partition_name, extracted.kernel_path
            )),
            config_text: extracted.config_text,
        }])
    }
}

struct PreparedImage {
    _download: Option<NamedTempFile>,
    image: PreparedLocalPath,
    source: String,
}

enum PreparedLocalPath {
    Borrowed(PathBuf),
    Temp(NamedTempFile),
}

impl PreparedLocalPath {
    fn path(&self) -> &Path {
        match self {
            Self::Borrowed(path) => path,
            Self::Temp(file) => file.path(),
        }
    }
}

struct ExtractedChromeOsConfig {
    partition_name: String,
    kernel_path: String,
    platform_version: String,
    kernel_version: String,
    config_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GptPartition {
    name: String,
    first_lba: u64,
    last_lba: u64,
}

fn extract_chromeos_config_from_image(image_path: &Path) -> Result<ExtractedChromeOsConfig> {
    let partitions = read_gpt_partitions(image_path)?;
    let root_partitions: Vec<_> = partitions
        .into_iter()
        .filter(|partition| partition.name.starts_with("ROOT-"))
        .collect();

    if root_partitions.is_empty() {
        bail!("no ROOT-* partition was found in {}", image_path.display());
    }

    let mut errors = Vec::new();
    for partition in root_partitions {
        match extract_from_root_partition(image_path, &partition) {
            Ok(extracted) => return Ok(extracted),
            Err(error) => errors.push(format!("{}: {error:#}", partition.name)),
        }
    }

    bail!(
        "failed to extract a ChromeOS kernel config from any ROOT-* partition in {}: {}",
        image_path.display(),
        errors.join("; ")
    );
}

fn extract_from_root_partition(
    image_path: &Path,
    partition: &GptPartition,
) -> Result<ExtractedChromeOsConfig> {
    let partition_image = extract_partition_to_temp(image_path, partition)?;
    let fs = Ext4::load_from_path(partition_image.path()).with_context(|| {
        format!(
            "loading ext filesystem from {} partition {}",
            image_path.display(),
            partition.name
        )
    })?;
    let platform_version = read_release_version(&fs).unwrap_or_else(|| "unknown".to_string());
    let mut errors = Vec::new();

    if let Some(kernel_path) = find_kernel_path(&fs)? {
        let kernel_image = fs
            .read(&kernel_path)
            .with_context(|| format!("reading {kernel_path} from {}", partition.name))?;
        let kernel_version = infer_kernel_version_from_path(&fs, &kernel_path)
            .unwrap_or_else(|| "unknown".to_string());
        match extract_ikconfig_from_image(&kernel_image).with_context(|| {
            format!(
                "extracting IKCONFIG from {kernel_path} in {}",
                partition.name
            )
        }) {
            Ok(config_text) => {
                return Ok(ExtractedChromeOsConfig {
                    partition_name: partition.name.clone(),
                    kernel_path,
                    platform_version,
                    kernel_version,
                    config_text,
                });
            }
            Err(error) => errors.push(error.to_string()),
        }
    }

    if let Some((module_path, kernel_version, config_text)) =
        extract_from_configs_module(&fs, &partition.name)?
    {
        return Ok(ExtractedChromeOsConfig {
            partition_name: partition.name.clone(),
            kernel_path: module_path,
            platform_version,
            kernel_version,
            config_text,
        });
    }

    if errors.is_empty() {
        bail!("no boot/vmlinuz or configs.ko module was found");
    }

    bail!("{}", errors.join("; "))
}

fn find_kernel_path(fs: &Ext4) -> Result<Option<String>> {
    let entries = match fs.read_dir("/boot") {
        Ok(entries) => entries,
        Err(_) => return Ok(None),
    };

    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry.context("reading /boot directory")?;
        let path: std::path::PathBuf = entry.path().into();
        let Some(name) = path.file_name() else {
            continue;
        };
        let name = name.to_string_lossy();
        if name == "vmlinuz" {
            candidates.push(path.to_string_lossy().to_string());
            continue;
        }
        if name.starts_with("vmlinuz") {
            candidates.push(path.to_string_lossy().to_string());
        }
    }

    candidates.sort_by(|left, right| {
        let left_plain = left.ends_with("/vmlinuz");
        let right_plain = right.ends_with("/vmlinuz");
        right_plain.cmp(&left_plain).then_with(|| left.cmp(right))
    });
    Ok(candidates.into_iter().next())
}

fn read_release_version(fs: &Ext4) -> Option<String> {
    for path in ["/etc/lsb-release", "/etc/os-release", "/usr/lib/os-release"] {
        let Ok(text) = fs.read_to_string(path) else {
            continue;
        };
        if let Some(value) = parse_release_value(&text, "CHROMEOS_RELEASE_VERSION") {
            return Some(value);
        }
        if let Some(value) = parse_release_value(&text, "VERSION_ID") {
            return Some(value);
        }
        if let Some(value) = parse_release_value(&text, "BUILD_ID") {
            return Some(value);
        }
    }
    None
}

fn extract_from_configs_module(
    fs: &Ext4,
    partition_name: &str,
) -> Result<Option<(String, String, String)>> {
    if let Some((module_path, kernel_version)) = find_configs_module_paths(fs)?.into_iter().next() {
        let module_bytes = fs
            .read(&module_path)
            .with_context(|| format!("reading {module_path} from {partition_name}"))?;
        let config_text = if module_path.ends_with(".gz") {
            let mut decoder = GzDecoder::new(Cursor::new(module_bytes));
            let mut inflated = Vec::new();
            decoder
                .read_to_end(&mut inflated)
                .with_context(|| format!("decompressing {module_path} from {partition_name}"))?;
            extract_ikconfig_from_image(&inflated).with_context(|| {
                format!("extracting IKCONFIG from decompressed {module_path} in {partition_name}")
            })?
        } else {
            extract_ikconfig_from_image(&module_bytes).with_context(|| {
                format!("extracting IKCONFIG from {module_path} in {partition_name}")
            })?
        };
        return Ok(Some((module_path, kernel_version, config_text)));
    }

    Ok(None)
}

fn find_configs_module_paths(fs: &Ext4) -> Result<Vec<(String, String)>> {
    let mut paths = Vec::new();
    let module_dirs = match fs.read_dir("/lib/modules") {
        Ok(entries) => entries,
        Err(_) => return Ok(paths),
    };

    for entry in module_dirs {
        let entry = entry.context("reading /lib/modules")?;
        let version_path: std::path::PathBuf = entry.path().into();
        let version_path = version_path.to_string_lossy().to_string();
        let kernel_kernel_dir = format!("{version_path}/kernel/kernel");
        let Ok(kernel_entries) = fs.read_dir(&kernel_kernel_dir) else {
            continue;
        };
        for kernel_entry in kernel_entries {
            let kernel_entry =
                kernel_entry.with_context(|| format!("reading {kernel_kernel_dir}"))?;
            let path: std::path::PathBuf = kernel_entry.path().into();
            let Some(name) = path.file_name() else {
                continue;
            };
            let name = name.to_string_lossy();
            if (name == "configs.ko" || name == "configs.ko.gz")
                && let Some(kernel_version) = kernel_version_from_module_path(&path)
            {
                paths.push((path.to_string_lossy().to_string(), kernel_version));
            }
        }
    }

    paths.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(paths)
}

fn infer_kernel_version_from_path(fs: &Ext4, path: &str) -> Option<String> {
    if let Some(version) = path
        .strip_prefix("/boot/vmlinuz-")
        .filter(|value| !value.is_empty())
    {
        return Some(version.to_string());
    }

    if path == "/boot/vmlinuz" {
        let target = fs.read_link(path).ok()?;
        let target: std::path::PathBuf = target.into();
        let target = target.to_string_lossy();
        return target
            .strip_prefix("vmlinuz-")
            .or_else(|| target.strip_prefix("/boot/vmlinuz-"))
            .map(str::to_string)
            .filter(|value| !value.is_empty());
    }

    None
}

fn kernel_version_from_module_path(path: &std::path::Path) -> Option<String> {
    let mut components = path.components();
    while let Some(component) = components.next() {
        if component.as_os_str() == "modules" {
            return components
                .next()
                .map(|value| value.as_os_str().to_string_lossy().to_string())
                .filter(|value| !value.is_empty());
        }
    }
    None
}

fn parse_release_value(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
        .map(|value| value.trim_matches('"').trim().to_string())
        .filter(|value| !value.is_empty())
}

fn read_gpt_partitions(image_path: &Path) -> Result<Vec<GptPartition>> {
    let mut file =
        File::open(image_path).with_context(|| format!("opening {}", image_path.display()))?;
    let mut header = [0u8; 512];
    file.seek(SeekFrom::Start(SECTOR_SIZE))
        .with_context(|| format!("seeking GPT header in {}", image_path.display()))?;
    file.read_exact(&mut header)
        .with_context(|| format!("reading GPT header from {}", image_path.display()))?;

    if &header[..8] != b"EFI PART" {
        bail!("{} did not contain a GPT header", image_path.display());
    }

    let partition_entry_lba = read_u64_le(&header, 72)?;
    let entry_count = read_u32_le(&header, 80)? as usize;
    let entry_size = read_u32_le(&header, 84)? as usize;
    if entry_size < 128 {
        bail!("GPT entry size {entry_size} was smaller than 128 bytes");
    }

    let table_offset = partition_entry_lba
        .checked_mul(SECTOR_SIZE)
        .context("GPT table offset overflow")?;
    file.seek(SeekFrom::Start(table_offset))
        .with_context(|| format!("seeking GPT table in {}", image_path.display()))?;

    let mut partitions = Vec::new();
    let mut entry = vec![0u8; entry_size];
    for _ in 0..entry_count {
        file.read_exact(&mut entry).with_context(|| {
            format!("reading GPT partition entry from {}", image_path.display())
        })?;
        if entry[..16].iter().all(|byte| *byte == 0) {
            continue;
        }

        let first_lba = read_u64_le(&entry, 32)?;
        let last_lba = read_u64_le(&entry, 40)?;
        if first_lba == 0 || last_lba < first_lba {
            continue;
        }

        let name = decode_gpt_name(&entry[56..128]);
        partitions.push(GptPartition {
            name,
            first_lba,
            last_lba,
        });
    }

    Ok(partitions)
}

fn decode_gpt_name(bytes: &[u8]) -> String {
    let mut code_units = Vec::new();
    for chunk in bytes.chunks_exact(2) {
        let unit = u16::from_le_bytes([chunk[0], chunk[1]]);
        if unit == 0 {
            break;
        }
        code_units.push(unit);
    }
    String::from_utf16_lossy(&code_units)
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32> {
    let range = bytes
        .get(offset..offset + 4)
        .with_context(|| format!("missing u32 at offset {offset}"))?;
    Ok(u32::from_le_bytes(
        range.try_into().expect("u32 slice length"),
    ))
}

fn read_u64_le(bytes: &[u8], offset: usize) -> Result<u64> {
    let range = bytes
        .get(offset..offset + 8)
        .with_context(|| format!("missing u64 at offset {offset}"))?;
    Ok(u64::from_le_bytes(
        range.try_into().expect("u64 slice length"),
    ))
}

async fn download_to_temp(client: &reqwest::Client, url: &str) -> Result<NamedTempFile> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("downloading {url}"))?;

    let mut temp = NamedTempFile::new().context("creating temporary download file")?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| format!("reading stream for {url}"))?;
        temp.write_all(&chunk)
            .with_context(|| format!("writing download for {url}"))?;
    }
    temp.flush()
        .with_context(|| format!("flushing download for {url}"))?;
    Ok(temp)
}

fn unzip_image_to_temp(zip_path: &Path) -> Result<PreparedLocalPath> {
    let entry = zip_payload_name(zip_path)?;
    let temp = NamedTempFile::new().context("creating temporary image file")?;
    let output_file = temp.reopen().context("reopening temporary image file")?;
    let status = Command::new("unzip")
        .arg("-p")
        .arg(zip_path)
        .arg(&entry)
        .stdout(Stdio::from(output_file))
        .status()
        .with_context(|| format!("running unzip for {}", zip_path.display()))?;
    if !status.success() {
        bail!(
            "unzip failed while extracting {entry} from {}",
            zip_path.display()
        );
    }
    Ok(PreparedLocalPath::Temp(temp))
}

fn zip_payload_name(zip_path: &Path) -> Result<String> {
    let output = Command::new("unzip")
        .arg("-Z1")
        .arg(zip_path)
        .output()
        .with_context(|| format!("listing ZIP archive {}", zip_path.display()))?;
    if !output.status.success() {
        bail!("failed to list ZIP archive {}", zip_path.display());
    }

    let listing = String::from_utf8(output.stdout).context("decoding unzip output")?;
    let mut entries: Vec<_> = listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.ends_with('/'))
        .map(str::to_string)
        .collect();
    entries.sort();

    if let Some(entry) = entries.iter().find(|entry| entry.ends_with(".bin")) {
        return Ok(entry.clone());
    }

    entries.into_iter().next().ok_or_else(|| {
        anyhow::anyhow!(
            "ZIP archive {} did not contain any files",
            zip_path.display()
        )
    })
}

fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("zip"))
        .unwrap_or(false)
}

fn extract_partition_to_temp(image_path: &Path, partition: &GptPartition) -> Result<NamedTempFile> {
    let start = partition
        .first_lba
        .checked_mul(SECTOR_SIZE)
        .context("partition start offset overflow")?;
    let end = partition
        .last_lba
        .checked_add(1)
        .and_then(|value| value.checked_mul(SECTOR_SIZE))
        .context("partition end offset overflow")?;
    if end <= start {
        bail!("partition {} had an invalid size", partition.name);
    }

    let mut source =
        File::open(image_path).with_context(|| format!("opening {}", image_path.display()))?;
    source.seek(SeekFrom::Start(start)).with_context(|| {
        format!(
            "seeking partition {} in {}",
            partition.name,
            image_path.display()
        )
    })?;
    let mut output = NamedTempFile::new().context("creating temporary partition image")?;
    let mut limited = source.take(end - start);
    std::io::copy(&mut limited, &mut output).with_context(|| {
        format!(
            "copying partition {} out of {}",
            partition.name,
            image_path.display()
        )
    })?;
    output
        .flush()
        .with_context(|| format!("flushing temporary partition image for {}", partition.name))?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    #[test]
    fn extracts_config_from_raw_recovery_image() {
        let temp = tempfile::tempdir().expect("tempdir");
        let image_path = build_test_recovery_image(temp.path()).expect("build recovery image");

        let extracted = extract_chromeos_config_from_image(&image_path).expect("extract config");

        assert_eq!(extracted.partition_name, "ROOT-A");
        assert_eq!(extracted.kernel_path, "/boot/vmlinuz");
        assert_eq!(extracted.platform_version, "16000.1.2");
        assert_eq!(extracted.kernel_version, "6.6.46-test");
        assert!(extracted.config_text.contains("CONFIG_CHROMEOS=y"));
    }

    #[test]
    fn falls_back_to_configs_module_when_vmlinuz_has_no_ikconfig() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root_dir = temp.path().join("rootdir");
        std::fs::create_dir_all(root_dir.join("boot")).expect("create boot dir");
        std::fs::create_dir_all(root_dir.join("etc")).expect("create etc dir");
        std::fs::create_dir_all(root_dir.join("lib/modules/1.0/kernel/kernel"))
            .expect("create module dir");
        std::fs::write(root_dir.join("boot/vmlinuz"), b"not-a-kernel-config").expect("vmlinuz");
        std::fs::write(
            root_dir.join("etc/lsb-release"),
            "CHROMEOS_RELEASE_VERSION=16000.1.2\n",
        )
        .expect("lsb-release");

        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&fake_ikconfig_image("CONFIG_MODULE_FALLBACK=y\n"))
            .expect("write module");
        let module = gz.finish().expect("finish module");
        std::fs::write(
            root_dir.join("lib/modules/1.0/kernel/kernel/configs.ko.gz"),
            module,
        )
        .expect("configs module");

        let rootfs_path = temp.path().join("rootfs.img");
        Command::new("truncate")
            .args(["-s", "64M"])
            .arg(&rootfs_path)
            .status()
            .expect("truncate")
            .success()
            .then_some(())
            .expect("truncate succeeded");
        Command::new("mkfs.ext4")
            .args(["-F", "-d"])
            .arg(&root_dir)
            .arg(&rootfs_path)
            .status()
            .expect("mkfs.ext4")
            .success()
            .then_some(())
            .expect("mkfs.ext4 succeeded");

        let image_path = temp.path().join("recovery.bin");
        write_test_gpt_image(&image_path, &rootfs_path).expect("write image");

        let extracted = extract_chromeos_config_from_image(&image_path).expect("extract config");
        assert_eq!(
            extracted.kernel_path,
            "/lib/modules/1.0/kernel/kernel/configs.ko.gz"
        );
        assert_eq!(extracted.platform_version, "16000.1.2");
        assert_eq!(extracted.kernel_version, "1.0");
        assert!(extracted.config_text.contains("CONFIG_MODULE_FALLBACK=y"));
    }

    #[test]
    fn parses_release_values() {
        assert_eq!(
            parse_release_value(
                "CHROMEOS_RELEASE_VERSION=16000.1.2\n",
                "CHROMEOS_RELEASE_VERSION"
            ),
            Some("16000.1.2".to_string())
        );
        assert_eq!(
            parse_release_value("VERSION_ID=\"123\"\n", "VERSION_ID"),
            Some("123".to_string())
        );
    }

    fn build_test_recovery_image(temp: &Path) -> Result<PathBuf> {
        let root_dir = temp.join("rootdir");
        std::fs::create_dir_all(root_dir.join("boot")).context("create boot dir")?;
        std::fs::create_dir_all(root_dir.join("etc")).context("create etc dir")?;
        let kernel_name = "vmlinuz-6.6.46-test";
        std::fs::write(
            root_dir.join(format!("boot/{kernel_name}")),
            fake_ikconfig_image("CONFIG_CHROMEOS=y\n# CONFIG_UNUSED is not set\n"),
        )
        .context("write vmlinuz")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(kernel_name, root_dir.join("boot/vmlinuz"))
            .context("write vmlinuz symlink")?;
        std::fs::write(
            root_dir.join("etc/lsb-release"),
            "CHROMEOS_RELEASE_VERSION=16000.1.2\n",
        )
        .context("write lsb-release")?;

        let rootfs_path = temp.join("rootfs.img");
        Command::new("truncate")
            .args(["-s", "64M"])
            .arg(&rootfs_path)
            .status()
            .context("running truncate")?
            .success()
            .then_some(())
            .ok_or_else(|| anyhow::anyhow!("truncate failed"))?;
        Command::new("mkfs.ext4")
            .args(["-F", "-d"])
            .arg(&root_dir)
            .arg(&rootfs_path)
            .status()
            .context("running mkfs.ext4")?
            .success()
            .then_some(())
            .ok_or_else(|| anyhow::anyhow!("mkfs.ext4 failed"))?;

        let image_path = temp.join("recovery.bin");
        write_test_gpt_image(&image_path, &rootfs_path)?;
        Ok(image_path)
    }

    fn write_test_gpt_image(image_path: &Path, rootfs_path: &Path) -> Result<()> {
        let rootfs = std::fs::read(rootfs_path)
            .with_context(|| format!("reading {}", rootfs_path.display()))?;
        let first_lba = 2048u64;
        let first_offset = first_lba * SECTOR_SIZE;
        let image_len = first_offset
            .checked_add(u64::try_from(rootfs.len()).context("rootfs len")?)
            .context("image length overflow")?;
        let mut image = vec![0u8; usize::try_from(image_len).context("image len usize")?];

        image[512..520].copy_from_slice(b"EFI PART");
        image[584..592].copy_from_slice(&2u64.to_le_bytes());
        image[592..596].copy_from_slice(&128u32.to_le_bytes());
        image[596..600].copy_from_slice(&128u32.to_le_bytes());

        let entry = &mut image[1024..1152];
        entry[..16].copy_from_slice(&[1u8; 16]);
        entry[32..40].copy_from_slice(&first_lba.to_le_bytes());
        let sector_len = u64::try_from(rootfs.len()).context("rootfs len u64")? / SECTOR_SIZE;
        entry[40..48].copy_from_slice(&(first_lba + sector_len - 1).to_le_bytes());
        for (index, unit) in "ROOT-A".encode_utf16().enumerate() {
            let offset = 56 + index * 2;
            entry[offset..offset + 2].copy_from_slice(&unit.to_le_bytes());
        }

        let start = usize::try_from(first_offset).context("rootfs start usize")?;
        image[start..start + rootfs.len()].copy_from_slice(&rootfs);
        std::fs::write(image_path, image)
            .with_context(|| format!("writing {}", image_path.display()))
    }

    fn fake_ikconfig_image(config: &str) -> Vec<u8> {
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(config.as_bytes()).expect("write gzip");
        let compressed = gz.finish().expect("finish gzip");

        let mut image = b"prefixIKCFG_ST".to_vec();
        image.extend_from_slice(&compressed);
        image.extend_from_slice(b"suffix");
        image
    }
}
