//! Configuration loading for nulld.
//!
//! Reads service definitions from /system/config/nulld.toml.

use crate::service::{RestartPolicy, ServiceDef};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_CONFIG_PATH: &str = "/system/config/nulld.toml";

#[derive(Debug, Deserialize)]
struct NulldConfig {
    #[serde(default)]
    service: HashMap<String, ServiceEntry>,
}

#[derive(Debug, Deserialize)]
struct ServiceEntry {
    binary: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(default = "default_restart")]
    restart: String,
}

fn default_restart() -> String {
    "always".to_string()
}

/// Load service definitions from the config file.
/// Falls back to built-in defaults if the config file doesn't exist.
pub fn load_services() -> Result<Vec<ServiceDef>, ConfigError> {
    let path = Path::new(DEFAULT_CONFIG_PATH);

    if !path.exists() {
        return Ok(builtin_services());
    }

    let content =
        std::fs::read_to_string(path).map_err(|e| ConfigError::ReadFailed {
            path: DEFAULT_CONFIG_PATH.to_string(),
            source: e,
        })?;

    parse_config(&content)
}

/// Parse a TOML config string into service definitions.
pub fn parse_config(content: &str) -> Result<Vec<ServiceDef>, ConfigError> {
    let config: NulldConfig =
        toml::from_str(content).map_err(|e| ConfigError::ParseFailed {
            source: e.to_string(),
        })?;

    let mut services = Vec::new();

    for (name, entry) in config.service {
        let restart = match entry.restart.as_str() {
            "always" => RestartPolicy::Always,
            "never" => RestartPolicy::Never,
            "on-failure" => RestartPolicy::OnFailure,
            other => {
                return Err(ConfigError::InvalidRestart {
                    service: name,
                    value: other.to_string(),
                });
            }
        };

        services.push(ServiceDef {
            name,
            binary: entry.binary,
            args: entry.args,
            depends_on: entry.depends_on,
            restart,
        });
    }

    Ok(services)
}

/// Built-in default services for NullBox v0.1.
/// Used when no config file is present (e.g., during early development).
fn builtin_services() -> Vec<ServiceDef> {
    vec![
        ServiceDef {
            name: "egress".to_string(),
            binary: "/system/bin/egress".to_string(),
            args: vec![],
            depends_on: vec![],
            restart: RestartPolicy::Always,
        },
        ServiceDef {
            name: "ctxgraph".to_string(),
            binary: "/system/bin/ctxgraph".to_string(),
            args: vec![],
            depends_on: vec![],
            restart: RestartPolicy::Always,
        },
        ServiceDef {
            name: "cage".to_string(),
            binary: "/system/bin/cage".to_string(),
            args: vec![],
            depends_on: vec!["egress".to_string(), "ctxgraph".to_string()],
            restart: RestartPolicy::Always,
        },
    ]
}

#[derive(Debug)]
pub enum ConfigError {
    ReadFailed {
        path: String,
        source: std::io::Error,
    },
    ParseFailed {
        source: String,
    },
    InvalidRestart {
        service: String,
        value: String,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadFailed { path, source } => {
                write!(f, "failed to read config {path}: {source}")
            }
            Self::ParseFailed { source } => {
                write!(f, "failed to parse config: {source}")
            }
            Self::InvalidRestart { service, value } => {
                write!(
                    f,
                    "invalid restart policy '{value}' for service '{service}'"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_config() {
        let toml = r#"
[service.egress]
binary = "/system/bin/egress"
args = []
depends_on = []
restart = "always"

[service.ctxgraph]
binary = "/system/bin/ctxgraph"
depends_on = []

[service.cage]
binary = "/system/bin/cage"
depends_on = ["egress", "ctxgraph"]
restart = "on-failure"
"#;
        let services = parse_config(toml).unwrap();
        assert_eq!(services.len(), 3);

        let cage = services.iter().find(|s| s.name == "cage").unwrap();
        assert_eq!(cage.depends_on, vec!["egress", "ctxgraph"]);
        assert_eq!(cage.restart, RestartPolicy::OnFailure);
    }

    #[test]
    fn parse_empty_config() {
        let services = parse_config("").unwrap();
        assert!(services.is_empty());
    }

    #[test]
    fn parse_invalid_restart_policy() {
        let toml = r#"
[service.bad]
binary = "/bin/bad"
restart = "bogus"
"#;
        let result = parse_config(toml);
        assert!(result.is_err());
    }

    #[test]
    fn builtin_services_have_correct_deps() {
        let services = builtin_services();
        let cage = services.iter().find(|s| s.name == "cage").unwrap();
        assert!(cage.depends_on.contains(&"egress".to_string()));
        assert!(cage.depends_on.contains(&"ctxgraph".to_string()));
    }
}
