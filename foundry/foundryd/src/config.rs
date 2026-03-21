use anyhow::{Context, Result};
use serde::Deserialize;

pub struct SecretValue;

impl SecretValue {
    pub fn resolve(value: &str) -> Result<String> {
        if value.starts_with("${") && value.ends_with('}') {
            let var_name = &value[2..value.len() - 1];
            std::env::var(var_name)
                .with_context(|| format!("Environment variable '{}' not set", var_name))
        } else {
            Ok(value.to_string())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub gitea: GiteaConfig,
    #[serde(default)]
    pub polling: PollingConfig,
    pub container: ContainerConfig,
    pub volumes: VolumesConfig,
    pub commands: CommandsConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub webhook_secret: String,
}

#[derive(Debug, Deserialize)]
pub struct GiteaConfig {
    pub url: String,
    pub api_token: String,
    pub bot_username: String,
    pub bot_display_name: String,
    pub bot_email: String,
    pub repos: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct PollingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_polling_interval")]
    pub interval_secs: u64,
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self { enabled: true, interval_secs: 120 }
    }
}

#[derive(Debug, Deserialize)]
pub struct ContainerConfig {
    pub image: String,
    pub runtime: String,
    pub network: String,
    pub memory_limit_mb: u64,
    pub cpu_limit: f64,
    pub max_concurrent: usize,
    pub timeout_secs: u64,
}

#[derive(Debug, Deserialize)]
pub struct VolumesConfig {
    pub issue_prefix: String,
    pub shared_volume: String,
}

#[derive(Debug, Deserialize)]
pub struct CommandsConfig {
    #[serde(default = "default_approve")]
    pub approve: String,
}

#[derive(Debug, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self { level: "info".into(), format: "json".into() }
    }
}

impl Config {
    pub fn from_toml(content: &str) -> Result<Self> {
        toml::from_str(content).context("Failed to parse config TOML")
    }

    pub fn from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;
        Self::from_toml(&content)
    }

    pub fn resolve_secrets(&mut self) -> Result<()> {
        self.server.webhook_secret =
            SecretValue::resolve(&self.server.webhook_secret)?;
        self.gitea.api_token =
            SecretValue::resolve(&self.gitea.api_token)?;
        Ok(())
    }
}

fn default_true() -> bool { true }
fn default_polling_interval() -> u64 { 120 }
fn default_approve() -> String { "/approve".into() }
fn default_log_level() -> String { "info".into() }
fn default_log_format() -> String { "json".into() }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parses_minimal_toml() {
        let toml = r#"
[server]
listen_addr = "0.0.0.0:8477"
webhook_secret = "mysecret"

[gitea]
url = "http://gitea.local"
api_token = "gta_abc"
bot_username = "foundry-bot"
bot_display_name = "Foundry Bot"
bot_email = "foundry-bot@local"

[container]
image = "foundry-runner:latest"
runtime = "docker"
network = "foundry-net"
memory_limit_mb = 1024
cpu_limit = 1.0
max_concurrent = 2
timeout_secs = 300

[volumes]
issue_prefix = "foundry-issue"
shared_volume = "foundry-shared"

[commands]
approve = "/approve"

[logging]
level = "info"
format = "json"
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.gitea.bot_username, "foundry-bot");
        assert_eq!(config.container.timeout_secs, 300);
        assert_eq!(config.commands.approve, "/approve");
    }

    #[test]
    fn config_requires_gitea_url() {
        let toml = r#"
[gitea]
api_token = "x"
bot_username = "bot"
bot_display_name = "Bot"
bot_email = "bot@local"
"#;
        assert!(Config::from_toml(toml).is_err());
    }

    #[test]
    fn secret_value_interpolates_env_var() {
        std::env::set_var("TEST_SECRET_XYZ", "my-secret-value");
        let result = SecretValue::resolve("${TEST_SECRET_XYZ}").unwrap();
        assert_eq!(result, "my-secret-value");
        std::env::remove_var("TEST_SECRET_XYZ");
    }

    #[test]
    fn secret_value_returns_literal_without_braces() {
        let result = SecretValue::resolve("plain-value").unwrap();
        assert_eq!(result, "plain-value");
    }
}
