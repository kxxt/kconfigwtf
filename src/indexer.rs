use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelConfigPackage {
    pub distribution: String,
    pub package_name: String,
    pub package_version: String,
    pub architecture: String,
    pub source: Option<String>,
    pub config_text: String,
}

#[async_trait]
pub trait KernelConfigIndexer: Send + Sync {
    async fn index(&self) -> Result<Vec<KernelConfigPackage>>;
}
