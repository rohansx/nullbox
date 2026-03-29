//! AGENT.toml manifest parser and validator.
//!
//! Every agent declares exactly what it needs. Cage reads the manifest
//! and creates a microVM with exactly these permissions — nothing more.

use serde::Deserialize;
use std::path::Path;

/// Parsed AGENT.toml manifest.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentManifest {
    pub agent: AgentMeta,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub tools: std::collections::HashMap<String, ToolDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentMeta {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
}

fn default_version() -> String {
    "0.0.0".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub network: NetworkCaps,
    #[serde(default)]
    pub filesystem: FilesystemCaps,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub credential_refs: Vec<String>,
    #[serde(default = "default_accelerator")]
    pub accelerator: String,
    #[serde(default = "default_cpu")]
    pub max_cpu_percent: u32,
    #[serde(default = "default_memory")]
    pub max_memory_mb: u32,
    #[serde(default)]
    pub max_api_calls_per_hour: u32,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            network: NetworkCaps::default(),
            filesystem: FilesystemCaps::default(),
            shell: false,
            credential_refs: Vec::new(),
            accelerator: default_accelerator(),
            max_cpu_percent: default_cpu(),
            max_memory_mb: default_memory(),
            max_api_calls_per_hour: 0,
        }
    }
}

fn default_accelerator() -> String {
    "none".to_string()
}

fn default_cpu() -> u32 {
    25
}

fn default_memory() -> u32 {
    256
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NetworkCaps {
    #[serde(default)]
    pub allow: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct FilesystemCaps {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolDef {
    pub risk: RiskLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Load and validate an AGENT.toml manifest from a file path.
pub fn load(path: &Path) -> Result<AgentManifest, ManifestError> {
    let content =
        std::fs::read_to_string(path).map_err(|e| ManifestError::ReadFailed {
            path: path.display().to_string(),
            source: e,
        })?;

    parse(&content)
}

/// Parse an AGENT.toml manifest from a string.
pub fn parse(content: &str) -> Result<AgentManifest, ManifestError> {
    let manifest: AgentManifest =
        toml::from_str(content).map_err(|e| ManifestError::ParseFailed {
            source: e.to_string(),
        })?;

    validate(&manifest)?;
    Ok(manifest)
}

/// Validate manifest constraints.
fn validate(manifest: &AgentManifest) -> Result<(), ManifestError> {
    if manifest.agent.name.is_empty() {
        return Err(ManifestError::Validation(
            "agent.name cannot be empty".to_string(),
        ));
    }

    if manifest.capabilities.max_cpu_percent > 100 {
        return Err(ManifestError::Validation(
            "max_cpu_percent cannot exceed 100".to_string(),
        ));
    }

    if manifest.capabilities.max_memory_mb == 0 {
        return Err(ManifestError::Validation(
            "max_memory_mb must be greater than 0".to_string(),
        ));
    }

    // Validate network allow entries are valid hostnames/domains
    for domain in &manifest.capabilities.network.allow {
        if domain.is_empty() || domain.contains(' ') {
            return Err(ManifestError::Validation(format!(
                "invalid domain in network.allow: '{domain}'"
            )));
        }
    }

    // Validate filesystem paths are absolute
    for path in manifest
        .capabilities
        .filesystem
        .read
        .iter()
        .chain(manifest.capabilities.filesystem.write.iter())
    {
        if !path.starts_with('/') {
            return Err(ManifestError::Validation(format!(
                "filesystem paths must be absolute: '{path}'"
            )));
        }
    }

    Ok(())
}

#[derive(Debug)]
pub enum ManifestError {
    ReadFailed {
        path: String,
        source: std::io::Error,
    },
    ParseFailed {
        source: String,
    },
    Validation(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReadFailed { path, source } => {
                write!(f, "failed to read manifest {path}: {source}")
            }
            Self::ParseFailed { source } => {
                write!(f, "failed to parse manifest: {source}")
            }
            Self::Validation(msg) => write!(f, "manifest validation: {msg}"),
        }
    }
}

impl std::error::Error for ManifestError {}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_MANIFEST: &str = r#"
[agent]
name = "researcher"
version = "1.2.0"

[capabilities]
shell = false
credential_refs = ["OPENAI_KEY", "EXA_KEY"]
accelerator = "none"
max_cpu_percent = 40
max_memory_mb = 512
max_api_calls_per_hour = 200

[capabilities.network]
allow = ["api.perplexity.ai", "api.exa.ai", "api.openai.com"]

[capabilities.filesystem]
read = ["/data/research"]
write = ["/data/research/output"]

[tools]
send_email = { risk = "low" }
read_files = { risk = "low" }
write_files = { risk = "medium" }
delete_files = { risk = "critical" }
execute_payment = { risk = "critical" }
"#;

    #[test]
    fn parse_full_manifest() {
        let manifest = parse(VALID_MANIFEST).unwrap();
        assert_eq!(manifest.agent.name, "researcher");
        assert_eq!(manifest.agent.version, "1.2.0");
        assert_eq!(manifest.capabilities.network.allow.len(), 3);
        assert_eq!(manifest.capabilities.max_memory_mb, 512);
        assert!(!manifest.capabilities.shell);
        assert_eq!(manifest.tools.len(), 5);
        assert_eq!(
            manifest.tools["delete_files"].risk,
            RiskLevel::Critical
        );
    }

    #[test]
    fn parse_minimal_manifest() {
        let toml = r#"
[agent]
name = "minimal"
"#;
        let manifest = parse(toml).unwrap();
        assert_eq!(manifest.agent.name, "minimal");
        assert_eq!(manifest.capabilities.max_cpu_percent, 25);
        assert_eq!(manifest.capabilities.max_memory_mb, 256);
    }

    #[test]
    fn reject_empty_name() {
        let toml = r#"
[agent]
name = ""
"#;
        assert!(parse(toml).is_err());
    }

    #[test]
    fn reject_cpu_over_100() {
        let toml = r#"
[agent]
name = "bad"

[capabilities]
max_cpu_percent = 150
"#;
        assert!(parse(toml).is_err());
    }

    #[test]
    fn reject_relative_filesystem_path() {
        let toml = r#"
[agent]
name = "bad"

[capabilities.filesystem]
read = ["relative/path"]
"#;
        assert!(parse(toml).is_err());
    }

    #[test]
    fn reject_invalid_domain() {
        let toml = r#"
[agent]
name = "bad"

[capabilities.network]
allow = ["valid.com", "has space"]
"#;
        assert!(parse(toml).is_err());
    }
}
