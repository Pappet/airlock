//! Configuration model for Airlock projects (`.airlock.json` or `.airlock.toml`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Project configuration for Airlock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Name of the project
    pub name: String,
    /// Schema version
    pub version: String,
    /// Defined session roles (e.g. "lead", "spec", "engine", "qa")
    #[serde(default)]
    pub roles: HashMap<String, RoleConfig>,
    /// Export targets (e.g. path to generated PLAN.md)
    #[serde(default)]
    pub export: ExportConfig,
}

/// Role configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleConfig {
    /// Human-readable description of the role's purpose
    pub description: String,
    /// Whether this role is subject to clean-room boundaries
    #[serde(default)]
    pub clean_room: bool,
}

/// Export configuration for syncing tasks to documentation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExportConfig {
    /// Path to generated plan markdown file
    pub plan_file: Option<String>,
    /// Path to generated questions markdown file
    pub questions_file: Option<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        let mut roles = HashMap::new();
        roles.insert(
            "lead".to_string(),
            RoleConfig {
                description: "System architecture and coordination".to_string(),
                clean_room: false,
            },
        );
        roles.insert(
            "spec".to_string(),
            RoleConfig {
                description: "Behavioral specification authoring".to_string(),
                clean_room: false,
            },
        );
        roles.insert(
            "engine".to_string(),
            RoleConfig {
                description: "Clean-room engine implementation".to_string(),
                clean_room: true,
            },
        );

        Self {
            name: "default".to_string(),
            version: "0.1.0".to_string(),
            roles,
            export: ExportConfig {
                plan_file: Some("docs/PLAN.md".to_string()),
                questions_file: Some("docs/spec/QUESTIONS.md".to_string()),
            },
        }
    }
}
