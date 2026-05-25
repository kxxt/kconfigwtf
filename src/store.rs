use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use walkdir::WalkDir;

use crate::ikconfig::extract_ikconfig_from_image;
use crate::index::{Architecture, Distribution};
use crate::indexer::{
    KernelConfigIndexer, KernelConfigPackage, normalize_nix_release_label, rolling_release_label,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorePackageManager {
    Nix { command: String, flake_ref: String },
    Guix { command: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePackageIndexerConfig {
    pub distribution: Distribution,
    pub release: String,
    pub manager: StorePackageManager,
    pub packages: Vec<String>,
    pub system: String,
    pub architecture: Architecture,
    pub max_packages: Option<usize>,
    pub skip_failed_packages: bool,
}

#[derive(Debug, Clone)]
pub struct StorePackageIndexer {
    config: StorePackageIndexerConfig,
}

impl StorePackageIndexer {
    pub fn new(config: StorePackageIndexerConfig) -> Self {
        Self { config }
    }
}

pub fn release_for_store_manager(
    distribution: &Distribution,
    manager: &StorePackageManager,
) -> String {
    match manager {
        StorePackageManager::Nix { flake_ref, .. }
            if matches!(distribution, Distribution::NixOS) =>
        {
            normalize_nix_release_label(flake_ref)
        }
        _ => rolling_release_label(),
    }
}

#[async_trait]
impl KernelConfigIndexer for StorePackageIndexer {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>> {
        let package_names = self
            .config
            .packages
            .iter()
            .take(self.config.max_packages.unwrap_or(usize::MAX));

        let mut packages = Vec::new();
        for package_name in package_names {
            let resolved = match resolve_store_package(
                &self.config.manager,
                package_name,
                &self.config.system,
            )
            .with_context(|| {
                format!(
                    "resolving {} package {package_name}",
                    self.config.distribution
                )
            }) {
                Ok(resolved) => resolved,
                Err(error) if self.config.skip_failed_packages => {
                    eprintln!(
                        "skipping {} package {package_name}: {error:#}",
                        self.config.distribution
                    );
                    continue;
                }
                Err(error) => return Err(error),
            };

            for store_path in resolved.store_paths {
                let configs =
                    extract_kernel_configs_from_store_path(&store_path).with_context(|| {
                        format!("extracting kernel config from {}", store_path.display())
                    })?;

                for (config_path, config_text) in configs {
                    packages.push(KernelConfigPackage {
                        distribution: self.config.distribution.clone(),
                        release: self.config.release.clone(),
                        package_name: package_name.clone(),
                        package_version: resolved.version.clone().unwrap_or_else(|| {
                            version_from_store_path(package_name, &store_path)
                                .unwrap_or_else(|| "unknown".to_string())
                        }),
                        architecture: self.config.architecture.clone(),
                        source: Some(format!("{}#{config_path}", store_path.display())),
                        config_text,
                    });
                }
            }
        }

        if packages.is_empty() {
            bail!(
                "{} store indexer did not find any kernel configs in {} package(s)",
                self.config.distribution,
                self.config.packages.len()
            );
        }

        Ok(packages)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedStorePackage {
    version: Option<String>,
    store_paths: Vec<PathBuf>,
}

fn resolve_store_package(
    manager: &StorePackageManager,
    package_name: &str,
    system: &str,
) -> Result<ResolvedStorePackage> {
    match manager {
        StorePackageManager::Nix { command, flake_ref } => {
            resolve_nix_package(command, flake_ref, package_name, system)
        }
        StorePackageManager::Guix { command } => {
            resolve_guix_package(command, package_name, system)
        }
    }
}

fn resolve_nix_package(
    command: &str,
    flake_ref: &str,
    package_name: &str,
    system: &str,
) -> Result<ResolvedStorePackage> {
    let installable = format!("{flake_ref}#{package_name}");
    let mut build_args = vec![
        OsString::from("build"),
        OsString::from("--no-link"),
        OsString::from("--print-out-paths"),
        OsString::from("--system"),
        OsString::from(system),
        OsString::from(&installable),
    ];
    let store_paths = parse_store_paths(&run_command(command, &build_args)?);

    let mut eval_args = vec![
        OsString::from("eval"),
        OsString::from("--raw"),
        OsString::from("--system"),
        OsString::from(system),
        OsString::from(format!("{installable}.version")),
    ];
    let version = run_command(command, &eval_args)
        .ok()
        .map(|output| output.trim().to_string())
        .filter(|output| !output.is_empty());

    build_args.clear();
    eval_args.clear();

    Ok(ResolvedStorePackage {
        version,
        store_paths,
    })
}

fn resolve_guix_package(
    command: &str,
    package_name: &str,
    system: &str,
) -> Result<ResolvedStorePackage> {
    let args = vec![
        OsString::from("build"),
        OsString::from(format!("--system={system}")),
        OsString::from(package_name),
    ];
    let store_paths = parse_store_paths(&run_command(command, &args)?);

    Ok(ResolvedStorePackage {
        version: store_paths
            .first()
            .and_then(|path| version_from_store_path(package_name, path)),
        store_paths,
    })
}

fn run_command(command: &str, args: &[OsString]) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .output()
        .with_context(|| format!("running {command}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{command} failed: {stderr}");
    }
    String::from_utf8(output.stdout).with_context(|| format!("decoding {command} stdout"))
}

fn parse_store_paths(output: &str) -> Vec<PathBuf> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('/') && !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

pub fn extract_kernel_configs_from_store_path(path: &Path) -> Result<Vec<(String, String)>> {
    let mut configs = Vec::new();
    let mut image_candidates = Vec::new();

    for entry in WalkDir::new(path).follow_links(false) {
        let entry = entry.with_context(|| format!("walking {}", path.display()))?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();
        if is_kernel_config_path(file_path) {
            let config_text = fs::read_to_string(file_path)
                .with_context(|| format!("reading kernel config {}", file_path.display()))?;
            configs.push((relative_store_path(path, file_path), config_text));
            continue;
        }

        if is_kernel_image_path(file_path) {
            image_candidates.push(file_path.to_path_buf());
        }
    }

    if configs.is_empty() {
        for image_path in image_candidates {
            let image = fs::read(&image_path)
                .with_context(|| format!("reading kernel image {}", image_path.display()))?;
            let Ok(config_text) = extract_ikconfig_from_image(&image) else {
                continue;
            };
            configs.push((relative_store_path(path, &image_path), config_text));
        }
    }

    configs.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(configs)
}

pub fn discover_nix_kernel_packages(
    command: &str,
    flake_ref: &str,
    system: &str,
) -> Result<Vec<String>> {
    let expression = r#"kernels: builtins.filter (name: let value = builtins.tryEval kernels.${name}; in value.success && builtins.isAttrs value.value && value.value ? type && value.value.type == "derivation") (builtins.attrNames kernels)"#;
    let args = vec![
        OsString::from("eval"),
        OsString::from("--json"),
        OsString::from("--system"),
        OsString::from(system),
        OsString::from("--apply"),
        OsString::from(expression),
        OsString::from(format!("{flake_ref}#linuxKernel.kernels")),
    ];
    let output = run_command(command, &args).context("discovering nixpkgs linuxKernel.kernels")?;
    let kernel_attrs: Vec<String> =
        serde_json::from_str(&output).context("parsing linuxKernel.kernels attribute names")?;

    Ok(nix_kernel_package_list(kernel_attrs))
}

fn nix_kernel_package_list(kernel_attrs: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut packages = BTreeSet::new();
    for attr in kernel_attrs {
        packages.insert(format!("linuxKernel.kernels.{attr}"));
    }

    packages.extend([
        "linuxPackages_latest.kernel".to_string(),
        "linux_zen".to_string(),
        "linux".to_string(),
        "linux_latest".to_string(),
        "linux_xanmod".to_string(),
    ]);

    packages.into_iter().collect()
}

fn relative_store_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn is_kernel_config_path(path: &Path) -> bool {
    let components = path_components(path);
    let Some(filename) = components.last() else {
        return false;
    };

    if filename == ".config" {
        return true;
    }

    if filename.starts_with("config-") {
        return true;
    }

    components.len() >= 3 && components[components.len() - 3] == "modules" && filename == "config"
}

fn is_kernel_image_path(path: &Path) -> bool {
    let Some(filename) = path
        .file_name()
        .map(|file_name| file_name.to_string_lossy())
    else {
        return false;
    };

    filename == "bzImage"
        || filename == "Image"
        || filename == "vmlinux"
        || filename.starts_with("vmlinuz-")
        || filename.starts_with("vmlinux-")
        || filename.starts_with("Image-")
        || filename.starts_with("bzImage-")
}

fn path_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_string())
        .collect()
}

fn version_from_store_path(package_name: &str, path: &Path) -> Option<String> {
    let basename = path.file_name()?.to_string_lossy();
    let name = basename.split_once('-')?.1;
    if let Some(version) = name.strip_prefix(&format!("{package_name}-")) {
        return Some(version.to_string());
    }

    name.split('-')
        .find(|segment| segment.starts_with(|character: char| character.is_ascii_digit()))
        .map(str::to_string)
}

pub fn default_system_for_architecture(architecture: &Architecture) -> String {
    match architecture {
        Architecture::Amd64 => "x86_64-linux",
        Architecture::Arm64 => "aarch64-linux",
        Architecture::Armhf => "armv7l-linux",
        Architecture::I386 => "i686-linux",
        Architecture::Ppc64el => "powerpc64le-linux",
        Architecture::Riscv64 => "riscv64-linux",
        Architecture::S390x => "s390x-linux",
        Architecture::Other(value) => value,
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;

    #[test]
    fn extracts_direct_config_from_store_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        let config_path = temp.path().join("lib/modules/6.12.0/build/.config");
        fs::create_dir_all(config_path.parent().expect("config parent")).expect("create dirs");
        fs::write(&config_path, "CONFIG_BPF=y\n").expect("write config");

        let configs = extract_kernel_configs_from_store_path(temp.path()).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "lib/modules/6.12.0/build/.config");
        assert!(configs[0].1.contains("CONFIG_BPF=y"));
    }

    #[test]
    fn extracts_embedded_config_from_store_kernel_image() {
        let temp = tempfile::tempdir().expect("tempdir");
        let image_path = temp.path().join("bzImage");
        fs::write(
            &image_path,
            fake_ikconfig_image("CONFIG_NIXOS=y\n# CONFIG_UNUSED is not set\n"),
        )
        .expect("write image");

        let configs = extract_kernel_configs_from_store_path(temp.path()).expect("extract configs");

        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].0, "bzImage");
        assert!(configs[0].1.contains("CONFIG_NIXOS=y"));
    }

    #[test]
    fn extracts_versions_from_store_paths() {
        assert_eq!(
            version_from_store_path(
                "linux-libre",
                Path::new("/gnu/store/abc-linux-libre-6.12.0")
            ),
            Some("6.12.0".to_string())
        );
        assert_eq!(
            version_from_store_path(
                "linuxPackages.kernel",
                Path::new("/nix/store/abc-linux-6.18.32")
            ),
            Some("6.18.32".to_string())
        );
    }

    #[test]
    fn maps_architectures_to_store_systems() {
        assert_eq!(
            default_system_for_architecture(&Architecture::Amd64),
            "x86_64-linux"
        );
        assert_eq!(
            default_system_for_architecture(&Architecture::Arm64),
            "aarch64-linux"
        );
    }

    #[test]
    fn builds_default_nix_kernel_package_list() {
        let packages = nix_kernel_package_list([
            "linux_5_10".to_string(),
            "linux_zen".to_string(),
            "linux_xanmod".to_string(),
        ]);

        assert!(packages.contains(&"linuxKernel.kernels.linux_5_10".to_string()));
        assert!(packages.contains(&"linuxKernel.kernels.linux_zen".to_string()));
        assert!(packages.contains(&"linuxPackages_latest.kernel".to_string()));
        assert!(packages.contains(&"linux".to_string()));
        assert!(packages.contains(&"linux_latest".to_string()));
        assert!(packages.contains(&"linux_xanmod".to_string()));
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
