use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use kconfigwtf::debian::{
    DebianIndexer, DebianIndexerConfig, DebianPackageBase, DebianPackageFeed, PackageIndexLocation,
};
use kconfigwtf::index::{Architecture, write_packages_to_data_dir};
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
    /// Index Debian kernel packages from a mirror or a local Packages file.
    Debian(DebianArgs),
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
            command: IndexCommand::Debian(args),
        } => index_debian(args).await,
        Command::Site(args) => generate_site(args),
    }
}

async fn index_debian(args: DebianArgs) -> Result<()> {
    let config = debian_config_from_args(&args)?;
    let indexer = DebianIndexer::new(config);
    let packages = indexer.index().await?;
    write_packages_to_data_dir(packages, &args.data_dir)
        .with_context(|| format!("writing data tree {}", args.data_dir.display()))?;
    Ok(())
}

fn debian_config_from_args(args: &DebianArgs) -> Result<DebianIndexerConfig> {
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
            feeds: vec![DebianPackageFeed {
                architecture,
                packages: PackageIndexLocation::Path(packages_file.clone()),
                deb_base: DebianPackageBase::Path(deb_root.clone()),
            }],
            package_name_prefix: args.package_prefix.clone(),
            max_packages: args.max_packages,
        }
    } else {
        DebianIndexerConfig::from_mirror(
            args.mirror.clone(),
            &args.suite,
            &args.component,
            args.architectures.clone(),
        )
    };

    config.package_name_prefix = args.package_prefix.clone();
    config.max_packages = args.max_packages;
    Ok(config)
}

fn generate_site(args: SiteArgs) -> Result<()> {
    SiteGenerator::new(args.title).generate(args.data_dir, args.output_dir)
}
