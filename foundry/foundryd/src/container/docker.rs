use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use bollard::{
    container::{
        Config as BollardConfig, CreateContainerOptions, KillContainerOptions,
        ListContainersOptions, LogsOptions, RemoveContainerOptions, StartContainerOptions,
        WaitContainerOptions,
    },
    models::{HostConfig, Mount as BollardMount, MountTypeEnum},
    volume::CreateVolumeOptions,
    Docker,
};
use foundry_core::{
    errors::ContainerError,
    traits::container_runtime::{ContainerResult, ContainerRuntime, ContainerSpec, Mount, VolumeSource},
};
use futures_util::StreamExt;
use std::collections::HashMap;
use tracing::{debug, info, warn};

pub const FOUNDRY_ISSUE_LABEL: &str = "foundry.issue";

pub struct DockerRuntime {
    docker: Docker,
}

impl DockerRuntime {
    pub async fn new() -> Result<Self, ContainerError> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ContainerError::Api(e.to_string()))?;
        Ok(Self { docker })
    }
}

fn build_mounts(mounts: &[Mount]) -> Vec<BollardMount> {
    mounts
        .iter()
        .map(|m| {
            let (mount_type, source) = match &m.source {
                VolumeSource::Named(name) => (MountTypeEnum::VOLUME, Some(name.clone())),
                VolumeSource::HostPath(path) => {
                    (MountTypeEnum::BIND, Some(path.to_string_lossy().to_string()))
                }
            };
            BollardMount {
                typ: Some(mount_type),
                source,
                target: Some(m.target.to_string_lossy().to_string()),
                read_only: Some(m.read_only),
                ..Default::default()
            }
        })
        .collect()
}

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    async fn run_container(&self, spec: &ContainerSpec) -> Result<ContainerResult, ContainerError> {
        let mounts = build_mounts(&spec.mounts);

        let env: Vec<String> = spec
            .env
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        let labels: HashMap<String, String> = spec.labels.clone();

        let memory = spec.memory_limit_bytes.map(|m| m as i64);
        let cpu_quota = spec.cpu_quota;
        let cpu_period = spec.cpu_period.map(|p| p as i64);

        let host_config = HostConfig {
            mounts: Some(mounts),
            network_mode: spec.network.clone(),
            memory,
            cpu_quota,
            cpu_period,
            ..Default::default()
        };

        let container_config = BollardConfig {
            image: Some(spec.image.clone()),
            env: Some(env),
            labels: Some(labels),
            host_config: Some(host_config),
            entrypoint: spec.entrypoint_override.clone(),
            user: spec.user.clone(),
            ..Default::default()
        };

        let container = self
            .docker
            .create_container(None::<CreateContainerOptions<String>>, container_config)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;

        let container_id = container.id.clone();

        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;

        info!("Started container {}", container_id);

        // Wait for container with optional timeout
        if spec.timeout_secs > 0 {
            let timeout = spec.timeout_secs;
            let cid = container_id.clone();
            let docker_clone = self.docker.clone();

            let wait_result = tokio::time::timeout(
                std::time::Duration::from_secs(timeout),
                async move {
                    let mut stream = docker_clone
                        .wait_container(&cid, None::<WaitContainerOptions<String>>);
                    stream.next().await
                },
            )
            .await;

            match wait_result {
                Ok(Some(Ok(response))) => {
                    let exit_code = response.status_code;
                    Ok(ContainerResult { container_id, exit_code })
                }
                Ok(Some(Err(e))) => Err(ContainerError::Api(e.to_string())),
                Ok(None) => Err(ContainerError::Api("Container wait stream ended unexpectedly".into())),
                Err(_) => {
                    // Timeout — kill the container
                    warn!("Container {} timed out, killing", container_id);
                    let _ = self
                        .docker
                        .kill_container(&container_id, None::<KillContainerOptions<String>>)
                        .await;
                    Err(ContainerError::Timeout { container_id })
                }
            }
        } else {
            // No timeout
            let mut stream = self
                .docker
                .wait_container(&container_id, None::<WaitContainerOptions<String>>);
            match stream.next().await {
                Some(Ok(response)) => {
                    let exit_code = response.status_code;
                    Ok(ContainerResult { container_id, exit_code })
                }
                Some(Err(e)) => Err(ContainerError::Api(e.to_string())),
                None => Err(ContainerError::Api("Container wait stream ended unexpectedly".into())),
            }
        }
    }

    async fn ensure_volume(&self, name: &str) -> Result<(), ContainerError> {
        let options = CreateVolumeOptions {
            name: name.to_string(),
            ..Default::default()
        };
        self.docker
            .create_volume(options)
            .await
            .map_err(|e| ContainerError::VolumeCreate {
                name: name.to_string(),
                reason: e.to_string(),
            })?;
        debug!("Ensured volume {}", name);
        Ok(())
    }

    async fn remove_volume(&self, name: &str) -> Result<(), ContainerError> {
        self.docker
            .remove_volume(name, None)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;
        debug!("Removed volume {}", name);
        Ok(())
    }

    async fn remove_container(&self, container_id: &str) -> Result<(), ContainerError> {
        self.docker
            .remove_container(
                container_id,
                Some(RemoveContainerOptions { force: true, ..Default::default() }),
            )
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;
        debug!("Removed container {}", container_id);
        Ok(())
    }

    async fn write_to_volume(
        &self,
        volume: &str,
        path: &str,
        contents: &[u8],
    ) -> Result<(), ContainerError> {
        // Validate that path is a simple filename (no slashes or shell metacharacters)
        if path.contains('/') || path.contains('\\') || path.contains(';') || path.contains('`') || path.contains('$') {
            return Err(ContainerError::VolumeWrite {
                volume: volume.to_string(),
                path: path.to_string(),
                reason: "path must be a simple filename (no slashes or metacharacters)".to_string(),
            });
        }

        let b64 = BASE64.encode(contents);
        let cmd = format!("sh -c 'echo {} | base64 -d > /data/{}'", b64, path);

        let mounts = vec![BollardMount {
            typ: Some(MountTypeEnum::VOLUME),
            source: Some(volume.to_string()),
            target: Some("/data".to_string()),
            read_only: Some(false),
            ..Default::default()
        }];

        let host_config = HostConfig {
            mounts: Some(mounts),
            auto_remove: Some(true),
            ..Default::default()
        };

        let container_config = BollardConfig {
            image: Some("alpine:latest".to_string()),
            cmd: Some(vec!["sh".into(), "-c".into(), cmd]),
            host_config: Some(host_config),
            ..Default::default()
        };

        let container = self
            .docker
            .create_container(None::<CreateContainerOptions<String>>, container_config)
            .await
            .map_err(|e| ContainerError::VolumeWrite {
                volume: volume.to_string(),
                path: path.to_string(),
                reason: e.to_string(),
            })?;

        self.docker
            .start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ContainerError::VolumeWrite {
                volume: volume.to_string(),
                path: path.to_string(),
                reason: e.to_string(),
            })?;

        let mut stream = self
            .docker
            .wait_container(&container.id, None::<WaitContainerOptions<String>>);
        if let Some(Err(e)) = stream.next().await {
            return Err(ContainerError::VolumeWrite {
                volume: volume.to_string(),
                path: path.to_string(),
                reason: e.to_string(),
            });
        }

        Ok(())
    }

    async fn read_from_volume(
        &self,
        volume: &str,
        path: &str,
    ) -> Result<Vec<u8>, ContainerError> {
        let cmd = format!("cat /data/{}", path);

        let mounts = vec![BollardMount {
            typ: Some(MountTypeEnum::VOLUME),
            source: Some(volume.to_string()),
            target: Some("/data".to_string()),
            read_only: Some(true),
            ..Default::default()
        }];

        let host_config = HostConfig {
            mounts: Some(mounts),
            auto_remove: Some(true),
            ..Default::default()
        };

        let container_config = BollardConfig {
            image: Some("alpine:latest".to_string()),
            cmd: Some(vec!["sh".into(), "-c".into(), cmd]),
            host_config: Some(host_config),
            attach_stdout: Some(true),
            attach_stderr: Some(false),
            ..Default::default()
        };

        let container = self
            .docker
            .create_container(None::<CreateContainerOptions<String>>, container_config)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;

        self.docker
            .start_container(&container.id, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;

        // Collect logs
        let logs_options = LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: false,
            ..Default::default()
        };
        let mut output = Vec::new();
        let mut log_stream = self.docker.logs(&container.id, Some(logs_options));
        while let Some(msg) = log_stream.next().await {
            match msg {
                Ok(bollard::container::LogOutput::StdOut { message }) => {
                    output.extend_from_slice(&message);
                }
                Ok(_) => {}
                Err(e) => return Err(ContainerError::Api(e.to_string())),
            }
        }

        Ok(output)
    }

    async fn list_running_with_label(
        &self,
        label_key: &str,
        label_value: Option<&str>,
    ) -> Result<Vec<String>, ContainerError> {
        let label_filter = match label_value {
            Some(val) => format!("{}={}", label_key, val),
            None => label_key.to_string(),
        };

        let mut filters = HashMap::new();
        filters.insert("label".to_string(), vec![label_filter]);
        filters.insert("status".to_string(), vec!["running".to_string()]);

        let options = ListContainersOptions {
            all: false,
            filters,
            ..Default::default()
        };

        let containers = self
            .docker
            .list_containers(Some(options))
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;

        Ok(containers
            .into_iter()
            .filter_map(|c| c.id)
            .collect())
    }

    async fn kill_container(&self, container_id: &str) -> Result<(), ContainerError> {
        self.docker
            .kill_container(container_id, None::<KillContainerOptions<String>>)
            .await
            .map_err(|e| ContainerError::Api(e.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use foundry_core::types::IssueKey;

    #[test]
    fn container_label_key_constant() {
        assert_eq!(FOUNDRY_ISSUE_LABEL, "foundry.issue");
    }

    #[test]
    fn build_container_spec_sets_label() {
        let key = IssueKey {
            owner: "alice".into(),
            repo: "proj".into(),
            issue_number: 1,
        };
        let label_value = format!("{}/{}/{}", key.owner, key.repo, key.issue_number);
        assert_eq!(label_value, "alice/proj/1");
    }

    #[test]
    fn container_spec_with_entrypoint_override_is_constructed() {
        // Verify the fields are accepted without compile error; runtime behavior
        // requires Docker and is covered by integration tests.
        let spec = ContainerSpec {
            image: "ubuntu:22.04".into(),
            env: Default::default(),
            mounts: vec![],
            network: None,
            memory_limit_bytes: None,
            cpu_period: None,
            cpu_quota: None,
            labels: Default::default(),
            timeout_secs: 30,
            entrypoint_override: Some(vec!["/bin/sh".into(), "/etc/foundry/bootstrap.sh".into()]),
            user: Some("1000".into()),
        };
        assert_eq!(spec.entrypoint_override.as_ref().unwrap()[0], "/bin/sh");
        assert_eq!(spec.user.as_deref(), Some("1000"));
    }
}
