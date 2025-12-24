use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs;

/// Configuration structures for RTSP services
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Config {
    pub services: Vec<ServiceConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ServiceConfig {
    #[serde(rename = "type")]
    pub svc_type: String,
    pub ip: String,
    pub port: u16,
    pub path: Option<String>,
    pub auth: Option<AuthConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct AuthConfig {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Auth {
    #[serde(rename = "type")]
    pub auth_type: String,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Service {
    #[serde(rename = "type")]
    pub svc_type: String,
    pub ip: String,
    pub port: u16,
    pub path: String,
    pub auth: Option<Auth>,
}

impl From<&ServiceConfig> for Service {
    fn from(config: &ServiceConfig) -> Self {
        Service {
            svc_type: config.svc_type.clone(),
            ip: config.ip.clone(),
            port: config.port,
            path: config.path.clone().unwrap_or_else(|| "/".to_string()),
            auth: config.auth.as_ref().map(|auth_cfg| Auth {
                auth_type: auth_cfg.auth_type.clone(),
                username: auth_cfg.username.clone(),
                password: auth_cfg.password.clone(),
            }),
        }
    }
}

/// Read and parse configuration from a file path
pub fn read_config<P: AsRef<std::path::Path>>(path: P) -> Result<Config> {
    let config_path = path.as_ref();
    let config_content = fs::read_to_string(config_path)
        .map_err(|e| anyhow::anyhow!("Failed to read config file '{:?}': {}", config_path, e))?;

    let config: Config = serde_json::from_str(&config_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse config file '{:?}': {}", config_path, e))?;

    if config.services.is_empty() {
        return Err(anyhow::anyhow!("No services found in config file"));
    }

    Ok(config)
}

/// Convert configuration to services vector with generated IDs
pub fn config_to_services(config: Config) -> Vec<(String, Service)> {
    config
        .services
        .into_iter()
        .enumerate()
        .map(|(index, service_config)| {
            // Generate ID based on service type and index if not provided
            let id = format!("{}-{}", service_config.svc_type, index + 1);
            (id, Service::from(&service_config))
        })
        .collect()
}
