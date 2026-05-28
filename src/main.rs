use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use kconfigwtf::alpine::{
    AlpineIndexer, AlpineIndexerConfig, AlpineRepoFeed, ApkIndexLocation, ApkPackageBase,
};
use kconfigwtf::android::{
    AndroidArtifactBase, AndroidGkiIndexer, AndroidGkiIndexerConfig, AndroidReleaseBuildsLocation,
    discover_release_build_branches,
};
use kconfigwtf::arch::{
    ArchDatabaseLocation, ArchIndexer, ArchIndexerConfig, ArchPackageBase, ArchRepoFeed,
    ArchRepositoryLayout,
};
use kconfigwtf::chromeos::{ChromeOsImageLocation, ChromeOsIndexer, ChromeOsIndexerConfig};
use kconfigwtf::debian::{
    DebianIndexer, DebianIndexerConfig, DebianPackageBase, DebianPackageFeed, PackageIndexLocation,
};
use kconfigwtf::fedora::{
    FedoraIndexer, FedoraIndexerConfig, FedoraMetadataLocation, FedoraPackageBase, FedoraRepoFeed,
};
use kconfigwtf::http::log_request_url;
use kconfigwtf::index::{
    Architecture, DEFAULT_MAX_INDEX_BYTES, Distribution, write_packages_to_data_dir,
};
use kconfigwtf::indexer::{
    normalize_alpine_release_label, normalize_apt_release_label, normalize_rpm_release_label,
    normalize_slackware_release_label, rolling_release_label,
};
use kconfigwtf::migration::migrate_data_dir;
use kconfigwtf::openwrt::{
    DEFAULT_TARGETS_URL as DEFAULT_OPENWRT_TARGETS_URL, OpenWrtIndexer, OpenWrtIndexerConfig,
    OpenWrtTargetsLocation,
};
use kconfigwtf::slackware::{
    SlackwareIndexLocation, SlackwareIndexer, SlackwareIndexerConfig, SlackwarePackageBase,
    SlackwareRepoFeed,
};
use kconfigwtf::store::{
    StorePackageIndexer, StorePackageIndexerConfig, StorePackageManager,
    default_system_for_architecture, discover_nix_kernel_packages, release_for_store_manager,
};
use kconfigwtf::void::{
    DEFAULT_VOID_GITHUB_RAW_SRCPKGS_URL, VoidIndexer, VoidIndexerConfig, VoidPackageBase,
    VoidRepoFeed,
};
use kconfigwtf::{KernelConfigIndexer, site::SiteGenerator};

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Retrieve kernel configs and write the data tree.
    Index {
        #[command(subcommand)]
        command: Box<IndexCommand>,
    },
    /// Migrate package indexes in a data directory to the compact schema.
    Migrate(MigrationArgs),
    /// Generate a static website from a data directory.
    Site(SiteArgs),
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    /// Index Android AOSP GKI kernel configs from release build metadata.
    Android(AndroidArgs),
    /// Index Alpine Linux kernel packages from an apk repository or local APKINDEX.
    Alpine(AlpineArgs),
    /// Index Arch Linux family kernel packages from a pacman repository or local sync database.
    Arch(ArchArgs),
    /// Index Debian kernel packages from a mirror or a local Packages file.
    Debian(DebianArgs),
    /// Index a ChromeOS recovery image by extracting IKCONFIG from boot/vmlinuz in a ROOT-* partition.
    #[command(name = "chromeos", alias = "chrome-os")]
    ChromeOS(ChromeOsArgs),
    /// Index eweOS kernel packages from a pacman repository or local sync database.
    #[command(name = "eweos", alias = "ewe-os")]
    EweOS(EweOsArgs),
    /// Index Fedora kernel packages from a repository or local repo metadata.
    Fedora(FedoraArgs),
    /// Index Red Hat Enterprise Linux kernel packages from RPM repository metadata.
    #[command(name = "rhel")]
    Rhel(RpmArgs),
    /// Index CentOS Stream kernel packages from RPM repository metadata.
    #[command(name = "centos")]
    CentOS(RpmArgs),
    /// Index AlmaLinux kernel packages from RPM repository metadata.
    #[command(name = "almalinux", alias = "alma", alias = "alma-linux")]
    AlmaLinux(RpmArgs),
    /// Index Kali Linux kernel packages from a mirror or a local Packages file.
    Kali(KaliArgs),
    /// Index openAnolis kernel packages from RPM repository metadata.
    #[command(name = "openanolis", alias = "open-anolis", alias = "anolis")]
    OpenAnolis(RpmArgs),
    /// Index openEuler kernel packages from RPM repository metadata.
    #[command(name = "openeuler", alias = "open-euler")]
    OpenEuler(RpmArgs),
    /// Index openSUSE kernel packages from RPM repository metadata.
    #[command(name = "opensuse", alias = "open-suse", alias = "suse")]
    OpenSUSE(RpmArgs),
    /// Index OpenWrt target configs from config.buildinfo and profiles.json.
    #[command(name = "openwrt", alias = "open-wrt")]
    OpenWrt(OpenWrtArgs),
    /// Index Proxmox VE kernel packages from a mirror or a local Packages file.
    Proxmox(ProxmoxArgs),
    /// Index Rocky Linux kernel packages from RPM repository metadata.
    #[command(name = "rocky", alias = "rockylinux", alias = "rocky-linux")]
    Rocky(RpmArgs),
    /// Index Ubuntu kernel packages from a mirror or a local Packages file.
    Ubuntu(UbuntuArgs),
    /// Index Deepin kernel packages from a mirror or a local Packages file.
    Deepin(DeepinArgs),
    /// Index Kylin OS kernel packages from a mirror or a local Packages file.
    #[command(name = "kylin", alias = "kylinos")]
    Kylin(KylinArgs),
    /// Index OpenKylin kernel packages from a mirror or a local Packages file.
    #[command(name = "openkylin", alias = "open-kylin")]
    OpenKylin(OpenKylinArgs),
    /// Index AOSC OS kernel packages from a mirror or a local Packages file.
    #[command(name = "aosc", alias = "aoscos", alias = "aosc-os")]
    AoscOS(AoscArgs),
    /// Index Oracle Linux kernel packages from RPM repository metadata.
    #[command(name = "oraclelinux", alias = "oracle", alias = "oracle-linux")]
    OracleLinux(RpmArgs),
    /// Index Amazon Linux kernel packages from RPM repository metadata.
    #[command(name = "amazonlinux", alias = "amazon", alias = "amazon-linux")]
    AmazonLinux(RpmArgs),
    /// Index Azure Linux kernel packages from RPM repository metadata.
    #[command(name = "azurelinux", alias = "azure", alias = "azure-linux")]
    AzureLinux(RpmArgs),
    /// Index Slackware kernel packages from a mirror or a local PACKAGES.TXT file.
    Slackware(SlackwareArgs),
    /// Index Void Linux kernel packages from a mirror or local package list.
    Void(VoidArgs),
    /// Index NixOS kernel packages through nix.
    #[command(name = "nixos", alias = "nix-os")]
    NixOS(NixOsArgs),
    /// Index Guix kernel packages through guix.
    Guix(GuixArgs),
}

#[derive(Debug, Args)]
struct AndroidArgs {
    /// Android GKI branch/package name to index. Repeat to index a selected subset.
    #[arg(long = "branch")]
    branches: Vec<String>,

    /// Source Android GKI overview URL used to discover branches when --branch is omitted.
    #[arg(
        long,
        default_value = "https://source.android.com/docs/core/architecture/kernel/gki1-overview"
    )]
    discovery_url: String,

    /// Local Source Android GKI overview page used to discover branches offline.
    #[arg(long)]
    discovery_file: Option<PathBuf>,

    /// Local directory containing release-build pages for discovered branches.
    ///
    /// Files are resolved as <release-builds-root>/gki-<branch-slug>-release-builds.json,
    /// falling back to .html.
    #[arg(long)]
    release_builds_root: Option<PathBuf>,

    /// Release builds JSON page URL. Defaults to the Source Android page for --branch.
    #[arg(long)]
    release_builds_url: Option<String>,

    /// Local release builds JSON or JSON HTML page. Useful for offline indexing and tests.
    #[arg(long)]
    release_builds_file: Option<PathBuf>,

    /// Local artifact root used with --release-builds-file.
    ///
    /// Files are resolved as <artifact-root>/<kernel_bid>/<target>/<config-artifact>.
    #[arg(long)]
    artifact_root: Option<PathBuf>,

    /// Android CI target containing the GKI artifacts.
    #[arg(long, default_value = "kernel_aarch64")]
    target: String,

    /// Artifact name containing the kernel .config.
    #[arg(long, default_value = "kernel_aarch64_dot_config")]
    config_artifact: String,

    /// CPU architecture to store for indexed configs.
    #[arg(long = "arch", default_value = "arm64")]
    architecture: Architecture,

    /// Limit the number of Android GKI builds fetched, newest first.
    #[arg(long)]
    max_builds: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct OpenWrtArgs {
    /// OpenWrt targets root URL used for remote indexing.
    ///
    /// Defaults to the snapshots target tree.
    #[arg(long)]
    targets_url: Option<String>,

    /// Local OpenWrt targets root. Useful for offline indexing and tests.
    #[arg(long)]
    targets_root: Option<PathBuf>,

    /// Target or target/subtarget to index. Repeat to select a subset.
    ///
    /// Examples: x86, x86/64, mediatek/mt7622.
    #[arg(long = "target")]
    targets: Vec<String>,

    /// Limit the number of discovered target/subtarget pairs indexed.
    #[arg(long)]
    max_targets: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ArchDistributionArg {
    #[value(alias = "arch", alias = "archlinux")]
    ArchLinux,
    Parabola,
    #[value(alias = "cachyos", alias = "cachy-os")]
    CachyOS,
}

impl ArchDistributionArg {
    fn distribution(self) -> Distribution {
        match self {
            Self::ArchLinux => Distribution::ArchLinux,
            Self::Parabola => Distribution::Parabola,
            Self::CachyOS => Distribution::CachyOS,
        }
    }

    fn default_mirror(self) -> &'static str {
        match self {
            Self::ArchLinux => "https://geo.mirror.pkgbuild.com",
            Self::Parabola => "https://repo.parabola.nu",
            Self::CachyOS => "https://cdn77.cachyos.org/repo/",
        }
    }

    fn default_repository(self) -> &'static str {
        match self {
            Self::ArchLinux => "core",
            Self::Parabola => "libre",
            Self::CachyOS => "cachyos-v3",
        }
    }

    fn default_architectures(self) -> Vec<Architecture> {
        vec![Architecture::Amd64]
    }

    fn repository_layout(self) -> ArchRepositoryLayout {
        match self {
            Self::CachyOS => ArchRepositoryLayout::ArchRepo,
            _ => ArchRepositoryLayout::RepoOsArch,
        }
    }
}

#[derive(Debug, Args)]
struct ArchArgs {
    /// Arch-family distribution to index.
    #[arg(long, value_enum, default_value_t = ArchDistributionArg::ArchLinux)]
    distribution: ArchDistributionArg,

    /// Pacman mirror root used for remote indexing.
    #[arg(long)]
    mirror: Option<String>,

    /// Pacman repository to index.
    #[arg(long)]
    repository: Option<String>,

    /// CPU architecture to index. May be passed more than once.
    ///
    /// Defaults to x86_64.
    #[arg(long = "arch")]
    architectures: Vec<Architecture>,

    /// Local pacman sync database file. Useful for offline indexing and tests.
    #[arg(long)]
    db_file: Option<PathBuf>,

    /// Local repository root used to resolve package filenames from --db-file.
    #[arg(long)]
    package_root: Option<PathBuf>,

    /// Package name prefix to include from the pacman sync database.
    #[arg(long, default_value = "linux")]
    package_prefix: String,

    /// Limit the number of pacman packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct EweOsArgs {
    /// eweOS mirror root used for remote indexing.
    #[arg(long, default_value = "https://os-repo.ewe.moe/eweos")]
    mirror: String,

    /// eweOS repository to index.
    #[arg(long, default_value = "main")]
    repository: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "x86_64")]
    architectures: Vec<Architecture>,

    /// Local pacman sync database file. Useful for offline indexing and tests.
    #[arg(long)]
    db_file: Option<PathBuf>,

    /// Local repository root used to resolve package filenames from --db-file.
    #[arg(long)]
    package_root: Option<PathBuf>,

    /// Package name prefix to include from the pacman sync database.
    #[arg(long, default_value = "linux")]
    package_prefix: String,

    /// Limit the number of pacman packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct AlpineArgs {
    /// Alpine mirror root used for remote indexing.
    #[arg(long, default_value = "https://dl-cdn.alpinelinux.org/alpine")]
    mirror: String,

    /// Alpine release to index.
    #[arg(long, default_value = "latest-stable")]
    release: String,

    /// Alpine repository to index. May be passed more than once.
    #[arg(long = "repository", default_values_t = vec!["main".to_string(), "community".to_string()])]
    repositories: Vec<String>,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "x86_64")]
    architectures: Vec<Architecture>,

    /// Local APKINDEX.tar.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    apkindex_file: Option<PathBuf>,

    /// Local repository root used to resolve .apk files from --apkindex-file.
    #[arg(long)]
    apk_root: Option<PathBuf>,

    /// Package name prefix to include from APKINDEX.
    #[arg(long, default_value = "linux-")]
    package_prefix: String,

    /// Limit the number of apk packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct DebianArgs {
    /// Debian mirror root used for remote indexing.
    #[arg(long, default_value = "https://deb.debian.org/debian")]
    mirror: String,

    /// Debian suite to index when using a mirror.
    #[arg(long, default_value = "stable")]
    suite: String,

    /// Debian archive component to index when using a mirror.
    #[arg(long, default_value = "main")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local Debian Packages or Packages.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the Debian Packages index.
    #[arg(long, default_value = "linux-image-")]
    package_prefix: String,

    /// Limit the number of Debian packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct UbuntuArgs {
    /// Ubuntu mirror root used for remote indexing.
    #[arg(long, default_value = "http://archive.ubuntu.com/ubuntu")]
    mirror: String,

    /// Ubuntu suite to index when using a mirror.
    #[arg(long, default_value = "noble-updates")]
    suite: String,

    /// Ubuntu archive component to index when using a mirror.
    #[arg(long, default_value = "main")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local Ubuntu Packages or Packages.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the Ubuntu Packages index.
    #[arg(long, default_value = "linux-modules-")]
    package_prefix: String,

    /// Limit the number of Ubuntu packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct KaliArgs {
    /// Kali mirror root used for remote indexing.
    #[arg(long, default_value = "http://http.kali.org/kali")]
    mirror: String,

    /// Kali suite to index when using a mirror.
    #[arg(long, default_value = "kali-rolling")]
    suite: String,

    /// Kali archive component to index when using a mirror.
    #[arg(long, default_value = "main")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local Kali Packages or Packages.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the Kali Packages index.
    #[arg(long, default_value = "linux-base-")]
    package_prefix: String,

    /// Limit the number of Kali packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct ProxmoxArgs {
    /// Proxmox mirror root used for remote indexing.
    #[arg(long, default_value = "http://download.proxmox.com/debian/pve")]
    mirror: String,

    /// Debian suite backing the Proxmox repository.
    #[arg(long, default_value = "bookworm")]
    suite: String,

    /// Proxmox repository component to index.
    #[arg(long, default_value = "pve-no-subscription")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local Proxmox Packages or Packages.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the Proxmox Packages index.
    #[arg(long, default_value = "proxmox-kernel-")]
    package_prefix: String,

    /// Limit the number of Proxmox packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct DeepinArgs {
    /// Deepin mirror root used for remote indexing.
    #[arg(long, default_value = "https://community-packages.deepin.com/beige")]
    mirror: String,

    /// Deepin suite to index when using a mirror.
    #[arg(long, default_value = "beige")]
    suite: String,

    /// Deepin archive component to index when using a mirror.
    #[arg(long, default_value = "main")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local Deepin Packages or Packages.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the Deepin Packages index.
    #[arg(long, default_value = "linux-image-")]
    package_prefix: String,

    /// Limit the number of Deepin packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct KylinArgs {
    /// Kylin mirror root used for remote indexing.
    #[arg(long, default_value = "https://archive.kylinos.cn/kylin/KYLIN-ALL")]
    mirror: String,

    /// Kylin suite to index when using a mirror.
    #[arg(long, default_value = "10.1")]
    suite: String,

    /// Kylin archive component to index when using a mirror.
    #[arg(long, default_value = "main")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local Kylin Packages or Packages.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the Kylin Packages index.
    #[arg(long, default_value = "linux-image-")]
    package_prefix: String,

    /// Limit the number of Kylin packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct OpenKylinArgs {
    /// OpenKylin mirror root used for remote indexing.
    #[arg(long, default_value = "https://archive.openkylin.top/openkylin")]
    mirror: String,

    /// OpenKylin suite to index when using a mirror.
    #[arg(long, default_value = "nile")]
    suite: String,

    /// OpenKylin archive component to index when using a mirror.
    #[arg(long, default_value = "main")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local OpenKylin Packages or Packages.gz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the OpenKylin Packages index.
    #[arg(long, default_value = "linux-modules-")]
    package_prefix: String,

    /// Limit the number of OpenKylin packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct AoscArgs {
    /// AOSC OS mirror root used for remote indexing.
    #[arg(long, default_value = "https://repo.aosc.io/debs")]
    mirror: String,

    /// AOSC OS suite to index when using a mirror.
    #[arg(long, default_value = "stable")]
    suite: String,

    /// AOSC OS archive component to index when using a mirror.
    #[arg(long, default_value = "main")]
    component: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Local AOSC OS Packages, Packages.gz, or Packages.xz file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve Filename fields from --packages-file.
    #[arg(long)]
    deb_root: Option<PathBuf>,

    /// Package name prefix to include from the AOSC OS Packages index.
    #[arg(long, default_value = "linux-kernel-")]
    package_prefix: String,

    /// Limit the number of AOSC OS packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct ChromeOsArgs {
    /// ChromeOS recovery image URL. Defaults to the public latest ChromeOS Flex recovery image.
    #[arg(
        long,
        default_value = "https://dl.google.com/chromeos-flex/images/latest.bin.zip"
    )]
    image_url: String,

    /// Local ChromeOS recovery image (.bin or .zip). Overrides --image-url.
    #[arg(long)]
    image_file: Option<PathBuf>,

    /// CPU architecture to store for indexed configs.
    #[arg(long = "arch", default_value = "amd64")]
    architecture: Architecture,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct NixOsArgs {
    /// Nix flake reference containing the package attributes.
    #[arg(long, default_value = "nixpkgs")]
    flake: String,

    /// Nix package attribute to index. May be passed more than once. Defaults to discovered kernels.
    #[arg(long = "package")]
    packages: Vec<String>,

    /// nix executable to run.
    #[arg(long, default_value = "nix")]
    nix_command: String,

    /// Nix system to build/evaluate. Defaults from --arch.
    #[arg(long)]
    system: Option<String>,

    /// CPU architecture to store for indexed configs.
    #[arg(long = "arch", default_value = "x86_64")]
    architecture: Architecture,

    /// Limit the number of Nix packages fetched.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct GuixArgs {
    /// Guix package name to index. May be passed more than once.
    #[arg(long = "package", default_values_t = vec!["linux-libre".to_string()])]
    packages: Vec<String>,

    /// guix executable to run.
    #[arg(long, default_value = "guix")]
    guix_command: String,

    /// Guix system to build. Defaults from --arch.
    #[arg(long)]
    system: Option<String>,

    /// CPU architecture to store for indexed configs.
    #[arg(long = "arch", default_value = "x86_64")]
    architecture: Architecture,

    /// Limit the number of Guix packages fetched.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct SlackwareArgs {
    /// Slackware mirror root used for remote indexing.
    #[arg(long, default_value = "https://mirrors.slackware.com/slackware")]
    mirror: String,

    /// Slackware release to index (e.g. slackware64-15.0, slackware64-current).
    #[arg(long, default_value = "slackware64-15.0")]
    release: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "x86_64")]
    architectures: Vec<Architecture>,

    /// Local PACKAGES.TXT file. Useful for offline indexing and tests.
    #[arg(long)]
    packages_file: Option<PathBuf>,

    /// Local root used to resolve package paths from --packages-file.
    #[arg(long)]
    package_root: Option<PathBuf>,

    /// Package name prefix to include from PACKAGES.TXT.
    #[arg(long, default_value = "kernel-")]
    package_prefix: String,

    /// Limit the number of packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct VoidArgs {
    /// Base URL for a raw `srcpkgs` tree when not using the default GitHub source.
    #[arg(long)]
    package_base: Option<String>,

    /// Local `void-packages` checkout root or `srcpkgs` directory.
    #[arg(long)]
    package_root: Option<PathBuf>,

    /// File containing package recipe names (one per line). Useful for offline indexing and tests.
    #[arg(long)]
    package_file: Option<PathBuf>,

    /// Explicit package recipe to index, for example `linux6.6`.
    #[arg(long = "package")]
    packages: Vec<String>,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "amd64")]
    architectures: Vec<Architecture>,

    /// Package recipe prefix to include from the package list.
    #[arg(long, default_value = "linux")]
    package_prefix: String,

    /// Limit the number of packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct FedoraArgs {
    /// Fedora mirror root used for remote indexing.
    #[arg(
        long,
        default_value = "https://download.fedoraproject.org/pub/fedora/linux"
    )]
    mirror: String,

    /// Fedora release to index. Use rawhide for Fedora development/rawhide.
    #[arg(long, default_value = "rawhide")]
    release: String,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "x86_64")]
    architectures: Vec<Architecture>,

    /// Local Fedora repodata/repomd.xml file. Useful for offline indexing and tests.
    #[arg(long)]
    repomd_file: Option<PathBuf>,

    /// Local repository root used to resolve repodata and RPM hrefs from --repomd-file.
    #[arg(long)]
    rpm_root: Option<PathBuf>,

    /// Fedora RPM package name to index.
    #[arg(long, default_value = "kernel-core")]
    package_name: String,

    /// Limit the number of Fedora packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct RpmArgs {
    /// RPM repository mirror root used for remote indexing.
    ///
    /// If omitted, a distribution-specific public mirror is used. RHEL uses the
    /// Red Hat CDN path and requires local entitlement or an accessible mirror.
    #[arg(long)]
    mirror: Option<String>,

    /// Distribution release to index. Defaults depend on the distribution.
    #[arg(long)]
    release: Option<String>,

    /// Repository to index. Defaults to BaseOS for EL distros and OS for openEuler.
    #[arg(long)]
    repository: Option<String>,

    /// CPU architecture to index. May be passed more than once.
    #[arg(long = "arch", default_value = "x86_64")]
    architectures: Vec<Architecture>,

    /// Local repodata/repomd.xml file. Useful for offline indexing and tests.
    #[arg(long)]
    repomd_file: Option<PathBuf>,

    /// Local repository root used to resolve repodata and RPM hrefs from --repomd-file.
    #[arg(long)]
    rpm_root: Option<PathBuf>,

    /// RPM package name to index. Defaults to kernel-core, or kernel for openEuler.
    #[arg(long)]
    package_name: Option<String>,

    /// Limit the number of RPM packages fetched per architecture.
    #[arg(long)]
    max_packages: Option<usize>,

    /// Output data directory.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Debug, Args)]
struct SiteArgs {
    /// Input data directory containing package indexes and raw configs.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,

    /// Static site output directory.
    #[arg(short, long, default_value = "public")]
    output_dir: PathBuf,

    /// Browser page title.
    #[arg(long, default_value = "kconfigwtf")]
    title: String,

    /// Number of worker threads for config page rendering. Defaults to available CPUs.
    #[arg(long)]
    jobs: Option<usize>,
}

#[derive(Debug, Args)]
struct MigrationArgs {
    /// Data directory containing distribution/package trees.
    #[arg(long, default_value = "data")]
    data_dir: PathBuf,

    /// Maximum pretty-printed bytes per index file before sharding.
    #[arg(long, default_value_t = DEFAULT_MAX_INDEX_BYTES)]
    max_index_bytes: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Index { command } => match *command {
            IndexCommand::Android(args) => index_android(args).await,
            IndexCommand::Alpine(args) => index_alpine(args).await,
            IndexCommand::Arch(args) => index_arch(args).await,
            IndexCommand::Debian(args) => index_debian(args).await,
            IndexCommand::ChromeOS(args) => index_chromeos(args).await,
            IndexCommand::EweOS(args) => index_eweos(args).await,
            IndexCommand::Fedora(args) => index_fedora(args).await,
            IndexCommand::Rhel(args) => index_rpm_distribution(Distribution::Rhel, args).await,
            IndexCommand::CentOS(args) => index_rpm_distribution(Distribution::CentOS, args).await,
            IndexCommand::AlmaLinux(args) => {
                index_rpm_distribution(Distribution::AlmaLinux, args).await
            }
            IndexCommand::Kali(args) => index_kali(args).await,
            IndexCommand::OpenAnolis(args) => {
                index_rpm_distribution(Distribution::OpenAnolis, args).await
            }
            IndexCommand::OpenEuler(args) => {
                index_rpm_distribution(Distribution::OpenEuler, args).await
            }
            IndexCommand::OpenSUSE(args) => {
                index_rpm_distribution(Distribution::OpenSUSE, args).await
            }
            IndexCommand::OpenWrt(args) => index_openwrt(args).await,
            IndexCommand::Proxmox(args) => index_proxmox(args).await,
            IndexCommand::Rocky(args) => index_rpm_distribution(Distribution::Rocky, args).await,
            IndexCommand::Ubuntu(args) => index_ubuntu(args).await,
            IndexCommand::Deepin(args) => index_deepin(args).await,
            IndexCommand::Kylin(args) => index_kylin(args).await,
            IndexCommand::OpenKylin(args) => index_openkylin(args).await,
            IndexCommand::AoscOS(args) => index_aosc(args).await,
            IndexCommand::NixOS(args) => index_nixos(args).await,
            IndexCommand::Guix(args) => index_guix(args).await,
            IndexCommand::OracleLinux(args) => {
                index_rpm_distribution(Distribution::OracleLinux, args).await
            }
            IndexCommand::AmazonLinux(args) => {
                index_rpm_distribution(Distribution::AmazonLinux, args).await
            }
            IndexCommand::AzureLinux(args) => {
                index_rpm_distribution(Distribution::AzureLinux, args).await
            }
            IndexCommand::Void(args) => index_void(args).await,
            IndexCommand::Slackware(args) => index_slackware(args).await,
        },
        Command::Migrate(args) => migrate(args),
        Command::Site(args) => generate_site(args),
    }
}

fn migrate(args: MigrationArgs) -> Result<()> {
    let summary = migrate_data_dir(&args.data_dir, args.max_index_bytes)
        .with_context(|| format!("migrating {}", args.data_dir.display()))?;
    eprintln!(
        "migrated {} package directories and wrote {} index files",
        summary.package_dirs,
        summary.index_files_written.len()
    );
    Ok(())
}

async fn index_android(args: AndroidArgs) -> Result<()> {
    let configs = android_configs_from_args(&args).await?;
    let mut packages = Vec::new();
    for config in configs {
        let indexer = AndroidGkiIndexer::new(config);
        packages.extend(indexer.index().await?);
    }
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_arch(args: ArchArgs) -> Result<()> {
    let config = arch_config_from_args(&args)?;
    let indexer = ArchIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_eweos(args: EweOsArgs) -> Result<()> {
    let config = eweos_config_from_args(&args)?;
    let indexer = ArchIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_alpine(args: AlpineArgs) -> Result<()> {
    let config = alpine_config_from_args(&args)?;
    let indexer = AlpineIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_debian(args: DebianArgs) -> Result<()> {
    let config = debian_config_from_args(&args)?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_fedora(args: FedoraArgs) -> Result<()> {
    let config = fedora_config_from_args(&args)?;
    let indexer = FedoraIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_rpm_distribution(distribution: Distribution, args: RpmArgs) -> Result<()> {
    let config = rpm_config_from_args(distribution, &args).await?;
    let indexer = FedoraIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_ubuntu(args: UbuntuArgs) -> Result<()> {
    let config = apt_config_from_args(AptConfigArgs {
        distribution: Distribution::Ubuntu,
        mirror: &args.mirror,
        suite: &args.suite,
        component: &args.component,
        architectures: &args.architectures,
        packages_file: args.packages_file.as_ref(),
        deb_root: args.deb_root.as_ref(),
        package_prefix: &args.package_prefix,
        required_package_substrings: &[],
        excluded_package_substrings: &[],
        max_packages: args.max_packages,
    })?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_kali(args: KaliArgs) -> Result<()> {
    let config = apt_config_from_args(AptConfigArgs {
        distribution: Distribution::Kali,
        mirror: &args.mirror,
        suite: &args.suite,
        component: &args.component,
        architectures: &args.architectures,
        packages_file: args.packages_file.as_ref(),
        deb_root: args.deb_root.as_ref(),
        package_prefix: &args.package_prefix,
        required_package_substrings: &[],
        excluded_package_substrings: &[],
        max_packages: args.max_packages,
    })?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_proxmox(args: ProxmoxArgs) -> Result<()> {
    let required = ["-pve".to_string()];
    let excluded = ["-signed".to_string(), "-signed-template".to_string()];
    let config = apt_config_from_args(AptConfigArgs {
        distribution: Distribution::Proxmox,
        mirror: &args.mirror,
        suite: &args.suite,
        component: &args.component,
        architectures: &args.architectures,
        packages_file: args.packages_file.as_ref(),
        deb_root: args.deb_root.as_ref(),
        package_prefix: &args.package_prefix,
        required_package_substrings: &required,
        excluded_package_substrings: &excluded,
        max_packages: args.max_packages,
    })?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_deepin(args: DeepinArgs) -> Result<()> {
    let config = apt_config_from_args(AptConfigArgs {
        distribution: Distribution::Deepin,
        mirror: &args.mirror,
        suite: &args.suite,
        component: &args.component,
        architectures: &args.architectures,
        packages_file: args.packages_file.as_ref(),
        deb_root: args.deb_root.as_ref(),
        package_prefix: &args.package_prefix,
        required_package_substrings: &[],
        excluded_package_substrings: &[],
        max_packages: args.max_packages,
    })?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_kylin(args: KylinArgs) -> Result<()> {
    let config = apt_config_from_args(AptConfigArgs {
        distribution: Distribution::Kylin,
        mirror: &args.mirror,
        suite: &args.suite,
        component: &args.component,
        architectures: &args.architectures,
        packages_file: args.packages_file.as_ref(),
        deb_root: args.deb_root.as_ref(),
        package_prefix: &args.package_prefix,
        required_package_substrings: &[],
        excluded_package_substrings: &[],
        max_packages: args.max_packages,
    })?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_openkylin(args: OpenKylinArgs) -> Result<()> {
    let config = apt_config_from_args(AptConfigArgs {
        distribution: Distribution::OpenKylin,
        mirror: &args.mirror,
        suite: &args.suite,
        component: &args.component,
        architectures: &args.architectures,
        packages_file: args.packages_file.as_ref(),
        deb_root: args.deb_root.as_ref(),
        package_prefix: &args.package_prefix,
        required_package_substrings: &[],
        excluded_package_substrings: &[],
        max_packages: args.max_packages,
    })?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_aosc(args: AoscArgs) -> Result<()> {
    let config = if let Some(packages_file) = &args.packages_file {
        let Some(deb_root) = &args.deb_root else {
            bail!("--deb-root is required when --packages-file is used");
        };

        let architecture = args
            .architectures
            .first()
            .cloned()
            .unwrap_or(Architecture::Amd64);

        DebianIndexerConfig {
            distribution: Distribution::AoscOS,
            release: normalize_apt_release_label(&args.suite),
            feeds: vec![DebianPackageFeed {
                architecture,
                packages: PackageIndexLocation::Path(packages_file.clone()),
                deb_base: DebianPackageBase::Path(deb_root.clone()),
            }],
            package_name_prefix: args.package_prefix.clone(),
            required_package_substrings: vec![],
            excluded_package_substrings: vec![],
            max_packages: args.max_packages,
        }
    } else {
        let mirror = args.mirror.trim_end_matches('/');
        let suite = &args.suite;
        let component = &args.component;

        let feeds = args
            .architectures
            .iter()
            .map(|architecture| {
                // Note: AOSC OS uses Packages.xz instead of Packages.gz
                let package_url =
                    format!("{mirror}/dists/{suite}/{component}/binary-{architecture}/Packages.xz");
                DebianPackageFeed {
                    architecture: architecture.clone(),
                    packages: PackageIndexLocation::Url(package_url),
                    deb_base: DebianPackageBase::Url(mirror.to_string()),
                }
            })
            .collect();

        DebianIndexerConfig {
            distribution: Distribution::AoscOS,
            release: normalize_apt_release_label(suite),
            feeds,
            package_name_prefix: args.package_prefix.clone(),
            required_package_substrings: vec![],
            excluded_package_substrings: vec![],
            max_packages: args.max_packages,
        }
    };

    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_chromeos(args: ChromeOsArgs) -> Result<()> {
    let indexer = ChromeOsIndexer::new(ChromeOsIndexerConfig {
        image: match args.image_file {
            Some(path) => ChromeOsImageLocation::Path(path),
            None => ChromeOsImageLocation::Url(args.image_url),
        },
        architecture: args.architecture,
    });
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_openwrt(args: OpenWrtArgs) -> Result<()> {
    if args.targets_root.is_some() && args.targets_url.is_some() {
        bail!("--targets-url and --targets-root are mutually exclusive");
    }

    let targets = match args.targets_root {
        Some(path) => OpenWrtTargetsLocation::Path(path),
        None => OpenWrtTargetsLocation::Url(
            args.targets_url
                .unwrap_or_else(|| DEFAULT_OPENWRT_TARGETS_URL.to_string()),
        ),
    };

    let indexer = OpenWrtIndexer::new(OpenWrtIndexerConfig {
        targets,
        selected_targets: args.targets,
        max_targets: args.max_targets,
    });
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_nixos(args: NixOsArgs) -> Result<()> {
    let system = args
        .system
        .unwrap_or_else(|| default_system_for_architecture(&args.architecture));
    let discovered_packages = args.packages.is_empty();
    let packages = if discovered_packages {
        discover_nix_kernel_packages(&args.nix_command, &args.flake, &system)?
    } else {
        args.packages
    };

    let config = StorePackageIndexerConfig {
        distribution: Distribution::NixOS,
        manager: StorePackageManager::Nix {
            command: args.nix_command,
            flake_ref: args.flake,
        },
        packages,
        system,
        architecture: args.architecture,
        max_packages: args.max_packages,
        skip_failed_packages: discovered_packages,
        release: String::new(),
    };
    let config = StorePackageIndexerConfig {
        release: release_for_store_manager(&config.distribution, &config.manager),
        ..config
    };
    let indexer = StorePackageIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_guix(args: GuixArgs) -> Result<()> {
    let config = StorePackageIndexerConfig {
        distribution: Distribution::Guix,
        manager: StorePackageManager::Guix {
            command: args.guix_command,
        },
        packages: args.packages,
        system: args
            .system
            .unwrap_or_else(|| default_system_for_architecture(&args.architecture)),
        architecture: args.architecture,
        max_packages: args.max_packages,
        skip_failed_packages: false,
        release: rolling_release_label(),
    };
    let indexer = StorePackageIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_slackware(args: SlackwareArgs) -> Result<()> {
    let config = slackware_config_from_args(&args)?;
    let indexer = SlackwareIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

async fn index_void(args: VoidArgs) -> Result<()> {
    let config = void_config_from_args(&args).await?;
    let indexer = VoidIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}
async fn void_config_from_args(args: &VoidArgs) -> Result<VoidIndexerConfig> {
    let mut package_names = if let Some(packages_file) = &args.package_file {
        let content = tokio::fs::read_to_string(packages_file)
            .await
            .with_context(|| format!("reading package list {}", packages_file.display()))?;
        content
            .lines()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    } else {
        args.packages.clone()
    };

    if package_names.is_empty() {
        if let Some(root) = &args.package_root {
            package_names = VoidIndexer::discover_packages_from_path(root).await?;
        } else {
            package_names = VoidIndexer::discover_packages_from_github().await?;
        }
    }

    if package_names.is_empty() {
        bail!(
            "no Void package recipes found; provide --package, --package-file, or a discoverable --package-root"
        );
    }

    let mut feeds = Vec::new();
    for architecture in &args.architectures {
        let package_base = if let Some(root) = &args.package_root {
            VoidPackageBase::Path(root.clone())
        } else {
            VoidPackageBase::Url(
                args.package_base
                    .clone()
                    .unwrap_or_else(|| DEFAULT_VOID_GITHUB_RAW_SRCPKGS_URL.to_string()),
            )
        };

        let mut filtered = package_names
            .iter()
            .filter(|name| name.starts_with(&args.package_prefix))
            .cloned()
            .collect::<Vec<_>>();
        filtered.sort();
        filtered.dedup();

        feeds.push(VoidRepoFeed {
            distribution: Distribution::Void,
            architecture: architecture.clone(),
            package_base: package_base.clone(),
            package_names: filtered,
        });
    }

    Ok(VoidIndexerConfig {
        release: rolling_release_label(),
        feeds,
        package_name_prefix: args.package_prefix.clone(),
        max_packages: args.max_packages,
    })
}

fn slackware_config_from_args(args: &SlackwareArgs) -> Result<SlackwareIndexerConfig> {
    if let Some(packages_file) = &args.packages_file {
        let Some(package_root) = &args.package_root else {
            bail!("--package-root is required when --packages-file is used");
        };

        let architecture = args
            .architectures
            .first()
            .cloned()
            .unwrap_or(Architecture::Amd64);

        return Ok(SlackwareIndexerConfig {
            release: normalize_slackware_release_label(&args.release),
            feeds: vec![SlackwareRepoFeed {
                distribution: Distribution::Slackware,
                architecture,
                packages_txt: SlackwareIndexLocation::Path(packages_file.clone()),
                package_base: SlackwarePackageBase::Path(package_root.clone()),
            }],
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
        });
    }

    let mirror = args.mirror.trim_end_matches('/').to_string();
    let release = &args.release;

    let feeds = args
        .architectures
        .iter()
        .map(|architecture| {
            let release_root = format!("{mirror}/{release}");
            SlackwareRepoFeed {
                distribution: Distribution::Slackware,
                architecture: architecture.clone(),
                packages_txt: SlackwareIndexLocation::Url(format!("{release_root}/PACKAGES.TXT")),
                package_base: SlackwarePackageBase::Url(release_root),
            }
        })
        .collect();

    Ok(SlackwareIndexerConfig {
        release: normalize_slackware_release_label(release),
        feeds,
        package_name_prefix: args.package_prefix.clone(),
        max_packages: args.max_packages,
    })
}

async fn android_configs_from_args(args: &AndroidArgs) -> Result<Vec<AndroidGkiIndexerConfig>> {
    if args.release_builds_url.is_some() && args.release_builds_file.is_some() {
        bail!("--release-builds-url and --release-builds-file are mutually exclusive");
    }
    if args.release_builds_root.is_some()
        && (args.release_builds_url.is_some() || args.release_builds_file.is_some())
    {
        bail!(
            "--release-builds-root cannot be combined with --release-builds-url or --release-builds-file"
        );
    }
    if (args.release_builds_url.is_some() || args.release_builds_file.is_some())
        && args.branches.len() > 1
    {
        bail!("explicit release-builds input can only be combined with one --branch");
    }

    let release_builds_locations = if let Some(path) = &args.release_builds_file {
        let Some(artifact_root) = &args.artifact_root else {
            bail!("--artifact-root is required when --release-builds-file is used");
        };
        vec![(
            args.branches
                .first()
                .cloned()
                .unwrap_or_else(|| "android".to_string()),
            AndroidReleaseBuildsLocation::Path(path.clone()),
            AndroidArtifactBase::Path(artifact_root.clone()),
        )]
    } else if let Some(url) = &args.release_builds_url {
        vec![(
            args.branches
                .first()
                .cloned()
                .unwrap_or_else(|| "android".to_string()),
            AndroidReleaseBuildsLocation::Url(url.clone()),
            android_artifact_base(args),
        )]
    } else if !args.branches.is_empty() {
        args.branches
            .iter()
            .map(|branch| {
                let config = AndroidGkiIndexerConfig::from_branch(branch.clone());
                let release_builds = if let Some(root) = &args.release_builds_root {
                    AndroidReleaseBuildsLocation::Path(android_release_builds_path(root, branch)?)
                } else {
                    config.release_builds.clone()
                };
                Ok((branch.clone(), release_builds, android_artifact_base(args)))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        let discovery = load_android_discovery(args).await?;
        let branches = discover_release_build_branches(&discovery);
        if branches.is_empty() {
            bail!("Android GKI discovery page did not contain any release-build branches");
        }
        branches
            .into_iter()
            .map(|branch| {
                let config = AndroidGkiIndexerConfig::from_branch(branch.clone());
                let release_builds = if let Some(root) = &args.release_builds_root {
                    AndroidReleaseBuildsLocation::Path(android_release_builds_path(root, &branch)?)
                } else {
                    config.release_builds.clone()
                };
                Ok((branch, release_builds, android_artifact_base(args)))
            })
            .collect::<Result<Vec<_>>>()?
    };

    release_builds_locations
        .into_iter()
        .map(|(branch, release_builds, artifact_base)| {
            let mut config = AndroidGkiIndexerConfig::from_branch(branch);
            config.release_builds = release_builds;
            config.artifact_base = artifact_base;
            config.target = args.target.clone();
            config.config_artifact = args.config_artifact.clone();
            config.architecture = args.architecture.clone();
            config.max_builds = args.max_builds;
            Ok(config)
        })
        .collect()
}

async fn load_android_discovery(args: &AndroidArgs) -> Result<String> {
    if let Some(path) = &args.discovery_file {
        return tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("reading Android GKI discovery page {}", path.display()));
    }

    log_request_url(&args.discovery_url);
    let response = reqwest::get(&args.discovery_url)
        .await
        .with_context(|| {
            format!(
                "requesting Android GKI discovery page {}",
                args.discovery_url
            )
        })?
        .error_for_status()
        .with_context(|| {
            format!(
                "Android GKI discovery page returned an error: {}",
                args.discovery_url
            )
        })?;
    response
        .text()
        .await
        .with_context(|| format!("reading Android GKI discovery page {}", args.discovery_url))
}

fn android_artifact_base(args: &AndroidArgs) -> AndroidArtifactBase {
    args.artifact_root
        .as_ref()
        .map(|root| AndroidArtifactBase::Path(root.clone()))
        .unwrap_or(AndroidArtifactBase::Ci)
}

fn android_release_builds_path(root: &std::path::Path, branch: &str) -> Result<PathBuf> {
    let slug = branch.replace('.', "_");
    let json_path = root.join(format!("gki-{slug}-release-builds.json"));
    if json_path.exists() {
        return Ok(json_path);
    }

    let html_path = root.join(format!("gki-{slug}-release-builds.html"));
    if html_path.exists() {
        return Ok(html_path);
    }

    bail!(
        "release builds file for {branch} was not found under {}",
        root.display()
    )
}

fn arch_config_from_args(args: &ArchArgs) -> Result<ArchIndexerConfig> {
    let distribution = args.distribution.distribution();
    let mut config = if let Some(db_file) = &args.db_file {
        let Some(package_root) = &args.package_root else {
            bail!("--package-root is required when --db-file is used");
        };

        let architecture = args.architectures.first().cloned().unwrap_or_else(|| {
            args.distribution
                .default_architectures()
                .into_iter()
                .next()
                .expect("arch default")
        });
        let include_kernel_packages = is_archlinux_riscv(&args.distribution, &architecture);

        ArchIndexerConfig {
            release: rolling_release_label(),
            feeds: vec![ArchRepoFeed {
                distribution,
                architecture,
                database: ArchDatabaseLocation::Path(db_file.clone()),
                package_base: ArchPackageBase::Path(package_root.clone()),
            }],
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
            include_kernel_packages,
        }
    } else {
        let repository = args
            .repository
            .clone()
            .unwrap_or_else(|| args.distribution.default_repository().to_string());
        let architectures = if args.architectures.is_empty() {
            args.distribution.default_architectures()
        } else {
            args.architectures.clone()
        };
        let feeds = architectures
            .iter()
            .flat_map(|architecture| {
                let is_archlinux_riscv = is_archlinux_riscv(&args.distribution, architecture);
                let mirror = args.mirror.clone().unwrap_or_else(|| {
                    if is_archlinux_riscv {
                        "https://archriscv.felixc.at/repo".to_string()
                    } else {
                        args.distribution.default_mirror().to_string()
                    }
                });
                let layout = if is_archlinux_riscv && args.mirror.is_none() {
                    ArchRepositoryLayout::RepoOnly
                } else {
                    args.distribution.repository_layout()
                };

                ArchIndexerConfig::from_mirror_with_layout(
                    distribution.clone(),
                    mirror,
                    repository.clone(),
                    [architecture.clone()],
                    layout,
                )
                .feeds
            })
            .collect::<Vec<_>>();

        ArchIndexerConfig {
            release: rolling_release_label(),
            feeds,
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
            include_kernel_packages: architectures
                .iter()
                .any(|architecture| is_archlinux_riscv(&args.distribution, architecture)),
        }
    };

    config.package_name_prefix = args.package_prefix.clone();
    config.max_packages = args.max_packages;
    config.include_kernel_packages = config.include_kernel_packages
        || config
            .feeds
            .iter()
            .any(|feed| is_archlinux_riscv(&args.distribution, &feed.architecture));
    Ok(config)
}

fn is_archlinux_riscv(distribution: &ArchDistributionArg, architecture: &Architecture) -> bool {
    matches!(distribution, ArchDistributionArg::ArchLinux) && architecture == &Architecture::Riscv64
}

fn eweos_config_from_args(args: &EweOsArgs) -> Result<ArchIndexerConfig> {
    let mut config = if let Some(db_file) = &args.db_file {
        let Some(package_root) = &args.package_root else {
            bail!("--package-root is required when --db-file is used");
        };

        let architecture = args
            .architectures
            .first()
            .cloned()
            .unwrap_or(Architecture::Amd64);

        ArchIndexerConfig {
            release: rolling_release_label(),
            feeds: vec![ArchRepoFeed {
                distribution: Distribution::EweOS,
                architecture,
                database: ArchDatabaseLocation::Path(db_file.clone()),
                package_base: ArchPackageBase::Path(package_root.clone()),
            }],
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
            include_kernel_packages: false,
        }
    } else {
        ArchIndexerConfig::from_mirror(
            Distribution::EweOS,
            args.mirror.clone(),
            &args.repository,
            args.architectures.clone(),
        )
    };

    config.package_name_prefix = args.package_prefix.clone();
    config.max_packages = args.max_packages;
    config.include_kernel_packages = false;
    Ok(config)
}

fn alpine_config_from_args(args: &AlpineArgs) -> Result<AlpineIndexerConfig> {
    let mut config = if let Some(apkindex_file) = &args.apkindex_file {
        let Some(apk_root) = &args.apk_root else {
            bail!("--apk-root is required when --apkindex-file is used");
        };

        let architecture = args
            .architectures
            .first()
            .cloned()
            .unwrap_or(Architecture::Amd64);

        AlpineIndexerConfig {
            release: normalize_alpine_release_label(&args.release),
            feeds: vec![AlpineRepoFeed {
                distribution: Distribution::Alpine,
                architecture,
                index: ApkIndexLocation::Path(apkindex_file.clone()),
                package_base: ApkPackageBase::Path(apk_root.clone()),
            }],
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
        }
    } else {
        let mirror = args.mirror.trim_end_matches('/').to_string();
        let mut feeds = Vec::new();
        for repository in &args.repositories {
            for architecture in &args.architectures {
                let repo_root = format!(
                    "{}/{}/{}/{}",
                    mirror,
                    args.release,
                    repository,
                    apk_architecture_segment(architecture)
                );
                feeds.push(AlpineRepoFeed {
                    distribution: Distribution::Alpine,
                    architecture: architecture.clone(),
                    index: ApkIndexLocation::Url(format!("{repo_root}/APKINDEX.tar.gz")),
                    package_base: ApkPackageBase::Url(repo_root),
                });
            }
        }

        AlpineIndexerConfig {
            release: normalize_alpine_release_label(&args.release),
            feeds,
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
        }
    };

    config.package_name_prefix = args.package_prefix.clone();
    config.max_packages = args.max_packages;
    Ok(config)
}

fn apk_architecture_segment(architecture: &Architecture) -> &str {
    match architecture {
        Architecture::Amd64 => "x86_64",
        Architecture::Arm64 => "aarch64",
        Architecture::Armhf => "armv7",
        Architecture::I386 => "x86",
        Architecture::Ppc64el => "ppc64le",
        other => other.as_str(),
    }
}

fn debian_config_from_args(args: &DebianArgs) -> Result<DebianIndexerConfig> {
    apt_config_from_args(AptConfigArgs {
        distribution: Distribution::Debian,
        mirror: &args.mirror,
        suite: &args.suite,
        component: &args.component,
        architectures: &args.architectures,
        packages_file: args.packages_file.as_ref(),
        deb_root: args.deb_root.as_ref(),
        package_prefix: &args.package_prefix,
        required_package_substrings: &[],
        excluded_package_substrings: &[],
        max_packages: args.max_packages,
    })
}

struct AptConfigArgs<'a> {
    distribution: Distribution,
    mirror: &'a str,
    suite: &'a str,
    component: &'a str,
    architectures: &'a [Architecture],
    packages_file: Option<&'a PathBuf>,
    deb_root: Option<&'a PathBuf>,
    package_prefix: &'a str,
    required_package_substrings: &'a [String],
    excluded_package_substrings: &'a [String],
    max_packages: Option<usize>,
}

fn apt_config_from_args(args: AptConfigArgs<'_>) -> Result<DebianIndexerConfig> {
    let mut config = if let Some(packages_file) = &args.packages_file {
        let Some(deb_root) = &args.deb_root else {
            bail!("--deb-root is required when --packages-file is used");
        };

        let architecture = args
            .architectures
            .first()
            .cloned()
            .unwrap_or(Architecture::Amd64);

        DebianIndexerConfig {
            distribution: args.distribution.clone(),
            release: normalize_apt_release_label(args.suite),
            feeds: vec![DebianPackageFeed {
                architecture,
                packages: PackageIndexLocation::Path((*packages_file).clone()),
                deb_base: DebianPackageBase::Path((*deb_root).clone()),
            }],
            package_name_prefix: args.package_prefix.to_string(),
            required_package_substrings: args.required_package_substrings.to_vec(),
            excluded_package_substrings: args.excluded_package_substrings.to_vec(),
            max_packages: args.max_packages,
        }
    } else {
        let mut config = DebianIndexerConfig::from_mirror(
            args.mirror.to_string(),
            args.suite,
            args.component,
            args.architectures.to_vec(),
        );
        config.distribution = args.distribution.clone();
        config
    };

    config.package_name_prefix = args.package_prefix.to_string();
    config.required_package_substrings = args.required_package_substrings.to_vec();
    config.excluded_package_substrings = args.excluded_package_substrings.to_vec();
    config.max_packages = args.max_packages;
    Ok(config)
}

fn fedora_config_from_args(args: &FedoraArgs) -> Result<FedoraIndexerConfig> {
    let mut config = if let Some(repomd_file) = &args.repomd_file {
        let Some(rpm_root) = &args.rpm_root else {
            bail!("--rpm-root is required when --repomd-file is used");
        };

        let architecture = args
            .architectures
            .first()
            .cloned()
            .unwrap_or(Architecture::Amd64);

        FedoraIndexerConfig {
            distribution: Distribution::Fedora,
            release: normalize_rpm_release_label(&Distribution::Fedora, &args.release),
            feeds: vec![FedoraRepoFeed {
                architecture,
                repomd: FedoraMetadataLocation::Path(repomd_file.clone()),
                package_base: FedoraPackageBase::Path(rpm_root.clone()),
            }],
            package_name: args.package_name.clone(),
            package_names: vec![args.package_name.clone()],
            max_packages: args.max_packages,
        }
    } else {
        FedoraIndexerConfig::from_mirror(
            args.mirror.clone(),
            &args.release,
            args.architectures.clone(),
        )
    };

    config.package_name = args.package_name.clone();
    config.package_names = vec![args.package_name.clone()];
    config.max_packages = args.max_packages;
    Ok(config)
}

async fn rpm_config_from_args(
    distribution: Distribution,
    args: &RpmArgs,
) -> Result<FedoraIndexerConfig> {
    let package_names = args.package_name.clone().map_or_else(
        || default_rpm_package_names(&distribution, args),
        |name| vec![name],
    );
    let package_name = package_names
        .first()
        .cloned()
        .expect("default RPM package names must not be empty");

    let mut config = if let Some(repomd_file) = &args.repomd_file {
        let Some(rpm_root) = &args.rpm_root else {
            bail!("--rpm-root is required when --repomd-file is used");
        };

        let architecture = args
            .architectures
            .first()
            .cloned()
            .unwrap_or(Architecture::Amd64);

        FedoraIndexerConfig {
            distribution: distribution.clone(),
            release: normalize_rpm_release_label(
                &distribution,
                args.release
                    .as_deref()
                    .unwrap_or(default_rpm_release(&distribution)),
            ),
            feeds: vec![FedoraRepoFeed {
                architecture,
                repomd: FedoraMetadataLocation::Path(repomd_file.clone()),
                package_base: FedoraPackageBase::Path(rpm_root.clone()),
            }],
            package_name: package_name.clone(),
            package_names: package_names.clone(),
            max_packages: args.max_packages,
        }
    } else {
        let mut feeds = Vec::new();
        for architecture in &args.architectures {
            let repo_root = rpm_repo_root(&distribution, args, architecture);
            let repo_root = if matches!(distribution, Distribution::AmazonLinux) {
                resolve_rpm_mirror_list(&repo_root).await?
            } else {
                repo_root
            };
            feeds.push(FedoraRepoFeed {
                architecture: architecture.clone(),
                repomd: FedoraMetadataLocation::Url(format!("{repo_root}/repodata/repomd.xml")),
                package_base: FedoraPackageBase::Url(repo_root),
            });
        }

        FedoraIndexerConfig {
            distribution: distribution.clone(),
            release: normalize_rpm_release_label(
                &distribution,
                args.release
                    .as_deref()
                    .unwrap_or(default_rpm_release(&distribution)),
            ),
            feeds,
            package_name: package_name.clone(),
            package_names: package_names.clone(),
            max_packages: args.max_packages,
        }
    };

    config.package_name = package_name;
    config.package_names = package_names;
    config.max_packages = args.max_packages;
    Ok(config)
}

async fn resolve_rpm_mirror_list(mirror_list_url: &str) -> Result<String> {
    log_request_url(mirror_list_url);
    let response = reqwest::get(mirror_list_url)
        .await
        .with_context(|| format!("fetching RPM mirror list from {mirror_list_url}"))?;
    let body = response.text().await?;
    let repo_url = body
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty mirror list from {mirror_list_url}"))?;
    Ok(repo_url.trim().trim_end_matches('/').to_string())
}

fn rpm_repo_root(
    distribution: &Distribution,
    args: &RpmArgs,
    architecture: &Architecture,
) -> String {
    let mirror = args
        .mirror
        .clone()
        .unwrap_or_else(|| default_rpm_mirror(distribution, args).to_string());
    let mut release = args
        .release
        .clone()
        .unwrap_or_else(|| default_rpm_release(distribution).to_string());
    if matches!(distribution, Distribution::CentOS) {
        release = canonical_centos_release(&release).to_string();
    }
    let repository = args
        .repository
        .clone()
        .unwrap_or_else(|| default_rpm_repository(distribution, &release).to_string());
    let mirror = mirror.trim_end_matches('/');
    let arch = rpm_architecture_segment(architecture);

    match distribution {
        Distribution::Rhel => {
            let major = release.split('.').next().unwrap_or(&release);
            format!(
                "{mirror}/rhel{major}/{release}/{arch}/{}/os",
                repository.to_ascii_lowercase()
            )
        }
        Distribution::CentOS if is_legacy_centos_release(&release) => {
            format!("{mirror}/{release}/{repository}/{arch}")
        }
        Distribution::CentOS | Distribution::AlmaLinux | Distribution::Rocky => {
            format!("{mirror}/{release}/{repository}/{arch}/os")
        }
        Distribution::OpenAnolis => format!("{mirror}/{release}/{repository}/{arch}/os"),
        Distribution::OpenEuler => format!("{mirror}/{release}/{repository}/{arch}"),
        Distribution::OpenSUSE if release == "tumbleweed" => {
            format!("{mirror}/tumbleweed/repo/{repository}")
        }
        Distribution::OpenSUSE => format!("{mirror}/distribution/leap/{release}/repo/{repository}"),
        Distribution::OracleLinux => {
            format!("{mirror}/OL{release}/{repository}/latest/{arch}")
        }
        Distribution::AmazonLinux => {
            format!("{mirror}/{release}/{repository}/mirrors/latest/{arch}/mirror.list")
        }
        Distribution::AzureLinux => {
            format!("{mirror}/{release}/{repository}/base/{arch}")
        }
        _ => format!("{mirror}/{release}/{repository}/{arch}/os"),
    }
}

fn default_rpm_mirror(distribution: &Distribution, args: &RpmArgs) -> &'static str {
    match distribution {
        Distribution::Rhel => "https://cdn.redhat.com/content/dist",
        Distribution::CentOS => {
            let release = args
                .release
                .as_deref()
                .unwrap_or(default_rpm_release(distribution));
            if is_archived_centos_release(canonical_centos_release(release)) {
                "https://vault.centos.org"
            } else {
                "https://mirror.stream.centos.org"
            }
        }
        Distribution::AlmaLinux => "https://repo.almalinux.org/almalinux",
        Distribution::Rocky => "https://dl.rockylinux.org/pub/rocky",
        Distribution::OpenAnolis => "https://mirrors.openanolis.cn/anolis",
        Distribution::OpenEuler => "https://repo.openeuler.org",
        Distribution::OpenSUSE => "https://download.opensuse.org",
        Distribution::OracleLinux => "https://yum.oracle.com/repo/OracleLinux",
        Distribution::AmazonLinux => "https://cdn.amazonlinux.com",
        Distribution::AzureLinux => "https://packages.microsoft.com/azurelinux",
        _ => "https://download.fedoraproject.org/pub/fedora/linux",
    }
}

fn default_rpm_release(distribution: &Distribution) -> &'static str {
    match distribution {
        Distribution::CentOS => "10-stream",
        Distribution::OpenAnolis => "23.1",
        Distribution::OpenEuler => "openEuler-24.03-LTS",
        Distribution::OpenSUSE => "tumbleweed",
        Distribution::OracleLinux => "9",
        Distribution::AmazonLinux => "al2023",
        Distribution::AzureLinux => "3.0",
        _ => "10",
    }
}

fn default_rpm_repository(distribution: &Distribution, release: &str) -> &'static str {
    match distribution {
        Distribution::CentOS if is_legacy_centos_release(release) => "os",
        Distribution::OpenAnolis if release.starts_with("8") => "kernel-5.10",
        Distribution::OpenAnolis => "os",
        Distribution::OpenEuler => "OS",
        Distribution::OpenSUSE => "oss",
        Distribution::OracleLinux => "baseos",
        Distribution::AmazonLinux => "core",
        Distribution::AzureLinux => "prod",
        _ => "BaseOS",
    }
}

fn default_rpm_package_name(distribution: &Distribution, args: &RpmArgs) -> &'static str {
    match distribution {
        Distribution::CentOS => {
            let release = args
                .release
                .as_deref()
                .unwrap_or(default_rpm_release(distribution));
            if is_legacy_centos_release(canonical_centos_release(release)) {
                "kernel"
            } else {
                "kernel-core"
            }
        }
        Distribution::OpenAnolis => {
            let release = args
                .release
                .as_deref()
                .unwrap_or(default_rpm_release(distribution));
            if release.starts_with("8") {
                "kernel-core"
            } else {
                "kernel"
            }
        }
        Distribution::OpenEuler => "kernel",
        Distribution::OpenSUSE => "kernel-default",
        Distribution::OracleLinux => "kernel-core",
        Distribution::AmazonLinux => "kernel",
        Distribution::AzureLinux => "kernel",
        _ => "kernel-core",
    }
}

fn default_rpm_package_names(distribution: &Distribution, args: &RpmArgs) -> Vec<String> {
    match distribution {
        Distribution::OpenSUSE => [
            "kernel-default",
            "kernel-vanilla",
            "kernel-longterm",
            "kernel-kvmsmall",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        _ => vec![default_rpm_package_name(distribution, args).to_string()],
    }
}

fn canonical_centos_release(release: &str) -> &str {
    match release {
        "6" => "6.10",
        "7" => "7.9.2009",
        "8" => "8.5.2111",
        other => other,
    }
}

fn is_legacy_centos_release(release: &str) -> bool {
    release
        .split(['.', '-'])
        .next()
        .and_then(|major| major.parse::<u16>().ok())
        .is_some_and(|major| major <= 7)
}

fn is_archived_centos_release(release: &str) -> bool {
    release
        .split(['.', '-'])
        .next()
        .and_then(|major| major.parse::<u16>().ok())
        .is_some_and(|major| major <= 8)
}

fn rpm_architecture_segment(architecture: &Architecture) -> &str {
    match architecture {
        Architecture::Amd64 => "x86_64",
        Architecture::Arm64 => "aarch64",
        Architecture::Armhf => "armhfp",
        Architecture::Ppc64el => "ppc64le",
        other => other.as_str(),
    }
}

fn generate_site(args: SiteArgs) -> Result<()> {
    let generator = SiteGenerator::new(args.title);
    let generator = if let Some(jobs) = args.jobs {
        if jobs == 0 {
            bail!("--jobs must be at least 1");
        }
        generator.with_parallelism(jobs)?
    } else {
        generator
    };
    generator.generate(args.data_dir, args.output_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_default_enterprise_linux_repo_roots() {
        let args = rpm_args();

        assert_eq!(
            rpm_repo_root(&Distribution::Rhel, &args, &Architecture::Amd64),
            "https://cdn.redhat.com/content/dist/rhel10/10/x86_64/baseos/os"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::CentOS, &args, &Architecture::Amd64),
            "https://mirror.stream.centos.org/10-stream/BaseOS/x86_64/os"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::AlmaLinux, &args, &Architecture::Arm64),
            "https://repo.almalinux.org/almalinux/10/BaseOS/aarch64/os"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::Rocky, &args, &Architecture::Ppc64el),
            "https://dl.rockylinux.org/pub/rocky/10/BaseOS/ppc64le/os"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::OpenEuler, &args, &Architecture::Amd64),
            "https://repo.openeuler.org/openEuler-24.03-LTS/OS/x86_64"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::OpenAnolis, &args, &Architecture::Amd64),
            "https://mirrors.openanolis.cn/anolis/23.1/os/x86_64/os"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::OpenSUSE, &args, &Architecture::Amd64),
            "https://download.opensuse.org/tumbleweed/repo/oss"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::OracleLinux, &args, &Architecture::Amd64),
            "https://yum.oracle.com/repo/OracleLinux/OL9/baseos/latest/x86_64"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::OracleLinux, &args, &Architecture::Arm64),
            "https://yum.oracle.com/repo/OracleLinux/OL9/baseos/latest/aarch64"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::AmazonLinux, &args, &Architecture::Amd64),
            "https://cdn.amazonlinux.com/al2023/core/mirrors/latest/x86_64/mirror.list"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::AzureLinux, &args, &Architecture::Amd64),
            "https://packages.microsoft.com/azurelinux/3.0/prod/base/x86_64"
        );
    }

    #[test]
    fn builds_legacy_centos_repo_roots_and_package_names() {
        let centos6 = rpm_args_with_release("6");
        let centos7 = rpm_args_with_release("7");
        let centos8 = rpm_args_with_release("8");
        let centos9_stream = rpm_args_with_release("9-stream");

        assert_eq!(
            rpm_repo_root(&Distribution::CentOS, &centos6, &Architecture::Amd64),
            "https://vault.centos.org/6.10/os/x86_64"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::CentOS, &centos7, &Architecture::Amd64),
            "https://vault.centos.org/7.9.2009/os/x86_64"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::CentOS, &centos8, &Architecture::Amd64),
            "https://vault.centos.org/8.5.2111/BaseOS/x86_64/os"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::CentOS, &centos9_stream, &Architecture::Amd64),
            "https://mirror.stream.centos.org/9-stream/BaseOS/x86_64/os"
        );
        assert_eq!(
            default_rpm_package_name(&Distribution::CentOS, &centos6),
            "kernel"
        );
        assert_eq!(
            default_rpm_package_name(&Distribution::CentOS, &centos8),
            "kernel-core"
        );
    }

    #[test]
    fn builds_alpine_feeds_for_main_and_community() {
        let args = AlpineArgs {
            mirror: "https://example.invalid/alpine".to_string(),
            release: "edge".to_string(),
            repositories: vec!["main".to_string(), "community".to_string()],
            architectures: vec![Architecture::Amd64],
            apkindex_file: None,
            apk_root: None,
            package_prefix: "linux-".to_string(),
            max_packages: None,
            data_dir: PathBuf::from("data"),
        };

        let config = alpine_config_from_args(&args).expect("alpine config");
        let urls = config
            .feeds
            .iter()
            .map(|feed| match &feed.index {
                ApkIndexLocation::Url(url) => url.as_str(),
                ApkIndexLocation::Path(_) => panic!("expected URL feed"),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            urls,
            vec![
                "https://example.invalid/alpine/edge/main/x86_64/APKINDEX.tar.gz",
                "https://example.invalid/alpine/edge/community/x86_64/APKINDEX.tar.gz",
            ]
        );
    }

    #[test]
    fn builds_archlinux_riscv64_feeds_from_arch_riscv_repository() {
        let args = ArchArgs {
            distribution: ArchDistributionArg::ArchLinux,
            mirror: None,
            repository: None,
            architectures: vec![Architecture::Riscv64],
            db_file: None,
            package_root: None,
            package_prefix: "linux".to_string(),
            max_packages: None,
            data_dir: PathBuf::from("data"),
        };

        let config = arch_config_from_args(&args).expect("arch linux riscv64 config");
        assert_eq!(config.feeds.len(), 1);
        assert_eq!(config.feeds[0].distribution, Distribution::ArchLinux);
        assert_eq!(config.feeds[0].architecture, Architecture::Riscv64);
        assert!(config.include_kernel_packages);
        assert_eq!(
            match &config.feeds[0].database {
                ArchDatabaseLocation::Url(url) => url.as_str(),
                ArchDatabaseLocation::Path(_) => panic!("expected URL database"),
            },
            "https://archriscv.felixc.at/repo/core/core.db"
        );
        assert_eq!(
            match &config.feeds[0].package_base {
                ArchPackageBase::Url(url) => url.as_str(),
                ArchPackageBase::Path(_) => panic!("expected URL package base"),
            },
            "https://archriscv.felixc.at/repo/core"
        );
    }

    #[test]
    fn builds_openanolis_and_opensuse_custom_release_repo_roots() {
        let anolis8 = RpmArgs {
            release: Some("8.10".to_string()),
            ..rpm_args()
        };
        let leap = RpmArgs {
            release: Some("15.6".to_string()),
            ..rpm_args()
        };

        assert_eq!(
            rpm_repo_root(&Distribution::OpenAnolis, &anolis8, &Architecture::Amd64),
            "https://mirrors.openanolis.cn/anolis/8.10/kernel-5.10/x86_64/os"
        );
        assert_eq!(
            default_rpm_package_name(&Distribution::OpenAnolis, &anolis8),
            "kernel-core"
        );
        assert_eq!(
            rpm_repo_root(&Distribution::OpenSUSE, &leap, &Architecture::Amd64),
            "https://download.opensuse.org/distribution/leap/15.6/repo/oss"
        );
        assert_eq!(
            default_rpm_package_name(&Distribution::OpenSUSE, &leap),
            "kernel-default"
        );
    }

    fn rpm_args() -> RpmArgs {
        RpmArgs {
            mirror: None,
            release: None,
            repository: None,
            architectures: vec![Architecture::Amd64],
            repomd_file: None,
            rpm_root: None,
            package_name: None,
            max_packages: None,
            data_dir: PathBuf::from("data"),
        }
    }

    fn rpm_args_with_release(release: &str) -> RpmArgs {
        RpmArgs {
            release: Some(release.to_string()),
            ..rpm_args()
        }
    }
}
