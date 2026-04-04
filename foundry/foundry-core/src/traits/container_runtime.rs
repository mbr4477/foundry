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
    /// Labels applied to the container (used for orphan detection on restart).
    pub labels: HashMap<String, String>,
    /// Kill the container after this many seconds (0 = no limit).
    pub timeout_secs: u64,
    /// Override the container image's entrypoint. If None, the image default is used.
    pub entrypoint_override: Option<Vec<String>>,
    /// Run the container as this user (e.g. "1000" or "foundry"). If None, image default is used.
    pub user: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_spec_entrypoint_and_user_default_to_none() {
        let spec = ContainerSpec {
            image: "ubuntu:22.04".into(),
            env: Default::default(),
            mounts: vec![],
            network: None,
            memory_limit_bytes: None,
            cpu_period: None,
            cpu_quota: None,
            labels: Default::default(),
            timeout_secs: 60,
            entrypoint_override: None,
            user: None,
        };
        assert!(spec.entrypoint_override.is_none());
        assert!(spec.user.is_none());
    }

    #[test]
    fn container_spec_stores_entrypoint_and_user() {
        let spec = ContainerSpec {
            image: "ubuntu:22.04".into(),
            env: Default::default(),
            mounts: vec![],
            network: None,
            memory_limit_bytes: None,
            cpu_period: None,
            cpu_quota: None,
            labels: Default::default(),
            timeout_secs: 60,
            entrypoint_override: Some(vec!["/bin/sh".into(), "/etc/foundry/bootstrap.sh".into()]),
            user: Some("1000".into()),
        };
        assert_eq!(
            spec.entrypoint_override.as_deref(),
            Some(&["/bin/sh".to_string(), "/etc/foundry/bootstrap.sh".to_string()][..])
        );
        assert_eq!(spec.user.as_deref(), Some("1000"));
    }
}
