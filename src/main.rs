use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use kconfigwtf::arch::{
    ArchDatabaseLocation, ArchIndexer, ArchIndexerConfig, ArchPackageBase, ArchRepoFeed,
};
use kconfigwtf::debian::{
    DebianIndexer, DebianIndexerConfig, DebianPackageBase, DebianPackageFeed, PackageIndexLocation,
};
use kconfigwtf::fedora::{
    FedoraIndexer, FedoraIndexerConfig, FedoraMetadataLocation, FedoraPackageBase, FedoraRepoFeed,
};
use kconfigwtf::index::{Architecture, Distribution, write_packages_to_data_dir};
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
        command: IndexCommand,
    },
    /// Generate a static website from a data directory.
    Site(SiteArgs),
}

#[derive(Debug, Subcommand)]
enum IndexCommand {
    /// Index Arch Linux family kernel packages from a pacman repository or local sync database.
    Arch(ArchArgs),
    /// Index Debian kernel packages from a mirror or a local Packages file.
    Debian(DebianArgs),
    /// Index Fedora kernel packages from a repository or local repo metadata.
    Fedora(FedoraArgs),
    /// Index Kali Linux kernel packages from a mirror or a local Packages file.
    Kali(KaliArgs),
    /// Index Proxmox VE kernel packages from a mirror or a local Packages file.
    Proxmox(ProxmoxArgs),
    /// Index Ubuntu kernel packages from a mirror or a local Packages file.
    Ubuntu(UbuntuArgs),
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
            Self::CachyOS => "https://mirror.cachyos.org/repo",
        }
    }

    fn default_repository(self) -> &'static str {
        match self {
            Self::ArchLinux => "core",
            Self::Parabola => "libre",
            Self::CachyOS => "cachyos-v3",
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
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Index {
            command: IndexCommand::Arch(args),
        } => index_arch(args).await,
        Command::Index {
            command: IndexCommand::Debian(args),
        } => index_debian(args).await,
        Command::Index {
            command: IndexCommand::Fedora(args),
        } => index_fedora(args).await,
        Command::Index {
            command: IndexCommand::Kali(args),
        } => index_kali(args).await,
        Command::Index {
            command: IndexCommand::Proxmox(args),
        } => index_proxmox(args).await,
        Command::Index {
            command: IndexCommand::Ubuntu(args),
        } => index_ubuntu(args).await,
        Command::Site(args) => generate_site(args),
    }
}

async fn index_arch(args: ArchArgs) -> Result<()> {
    let config = arch_config_from_args(&args)?;
    let indexer = ArchIndexer::new(config);
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

fn arch_config_from_args(args: &ArchArgs) -> Result<ArchIndexerConfig> {
    let distribution = args.distribution.distribution();
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
            feeds: vec![ArchRepoFeed {
                distribution,
                architecture,
                database: ArchDatabaseLocation::Path(db_file.clone()),
                package_base: ArchPackageBase::Path(package_root.clone()),
            }],
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
        }
    } else {
        let mirror = args
            .mirror
            .clone()
            .unwrap_or_else(|| args.distribution.default_mirror().to_string());
        let repository = args
            .repository
            .clone()
            .unwrap_or_else(|| args.distribution.default_repository().to_string());
        ArchIndexerConfig::from_mirror(distribution, mirror, repository, args.architectures.clone())
    };

    config.package_name_prefix = args.package_prefix.clone();
    config.max_packages = args.max_packages;
    Ok(config)
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
            feeds: vec![FedoraRepoFeed {
                architecture,
                repomd: FedoraMetadataLocation::Path(repomd_file.clone()),
                package_base: FedoraPackageBase::Path(rpm_root.clone()),
            }],
            package_name: args.package_name.clone(),
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
    config.max_packages = args.max_packages;
    Ok(config)
}

fn generate_site(args: SiteArgs) -> Result<()> {
    SiteGenerator::new(args.title).generate(args.data_dir, args.output_dir)
}
