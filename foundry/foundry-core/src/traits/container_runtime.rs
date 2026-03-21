use crate::errors::ContainerError;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ContainerSpec {
    pub image: String,
    pub env: HashMap<String, String>,
    pub mounts: Vec<Mount>,
    pub network: Option<String>,
    pub memory_limit_bytes: Option<u64>,
    pub cpu_period: Option<u64>,
    pub cpu_quota: Option<i64>,
    pub labels: HashMap<String, String>,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct Mount {
    pub source: VolumeSource,
    pub target: PathBuf,
    pub read_only: bool,
}

#[derive(Debug, Clone)]
pub enum VolumeSource {
    Named(String),
    HostPath(PathBuf),
}

#[derive(Debug)]
pub struct ContainerResult {
    pub container_id: String,
    pub exit_code: i64,
}

#[async_trait]
pub trait ContainerRuntime: Send + Sync + 'static {
    async fn run_container(&self, spec: &ContainerSpec) -> Result<ContainerResult, ContainerError>;
    async fn ensure_volume(&self, name: &str) -> Result<(), ContainerError>;
    async fn remove_volume(&self, name: &str) -> Result<(), ContainerError>;
    async fn remove_container(&self, container_id: &str) -> Result<(), ContainerError>;
    async fn write_to_volume(
        &self,
        volume: &str,
        path: &str,
        contents: &[u8],
    ) -> Result<(), ContainerError>;
    async fn read_from_volume(
        &self,
        volume: &str,
        path: &str,
    ) -> Result<Vec<u8>, ContainerError>;
    async fn list_running_with_label(
        &self,
        label_key: &str,
        label_value: Option<&str>,
    ) -> Result<Vec<String>, ContainerError>;
    async fn kill_container(&self, container_id: &str) -> Result<(), ContainerError>;
}
