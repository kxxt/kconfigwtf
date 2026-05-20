use anyhow::Result;
use async_trait::async_trait;

use crate::index::{Architecture, Distribution};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelConfigPackage {
    pub distribution: Distribution,
    pub package_name: String,
    pub package_version: String,
    pub architecture: Architecture,
    pub source: Option<String>,
    pub config_text: String,
}

#[async_trait]
pub trait KernelConfigIndexer: Send + Sync {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>>;
}
