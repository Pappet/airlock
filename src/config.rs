//! Project configuration, stored hub-side as `project.json`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Current `project.json` schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// Project configuration for Airlock.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    /// `project.json` schema version
    pub schema_version: u32,
    /// Human-readable project name (usable with `--project`)
    pub name: String,
    /// Defined session roles (e.g. "lead", "spec", "engine", "qa")
    #[serde(default)]
    pub roles: BTreeMap<String, RoleConfig>,
    /// Default lease duration for task claims, in minutes
    #[serde(default = "default_lease_minutes")]
    pub lease_minutes: u32,
    /// Export targets written by `airlock sync`
    #[serde(default)]
    pub export: ExportConfig,
}

/// Role configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleConfig {
    /// Human-readable description of the role's purpose
    pub description: String,
    /// Whether sessions bound to this role get the restricted clean-room tool set
    #[serde(default)]
    pub clean_room: bool,
}

/// Export paths, relative to the project root that invokes `airlock sync`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ExportConfig {
    /// Canonical JSON state snapshot (always written)
    pub state_file: String,
    /// Generated plan view; `None` disables it
    pub plan_file: Option<String>,
    /// Generated question log; `None` disables it
    pub questions_file: Option<String>,
}

fn default_lease_minutes() -> u32 {
    60
}

impl Default for ExportConfig {
    fn default() -> Self {
        Self {
            state_file: "docs/.airlock-state.json".to_string(),
            plan_file: Some("docs/PLAN.md".to_string()),
            questions_file: Some("docs/spec/QUESTIONS.md".to_string()),
        }
    }
}

impl Default for ProjectConfig {
    fn default() -> Self {
        let role = |description: &str, clean_room| RoleConfig {
            description: description.to_string(),
            clean_room,
        };
        let roles = BTreeMap::from([
            (
                "lead".to_string(),
                role("System architecture and coordination", false),
            ),
            (
                "spec".to_string(),
                role("Behavioral specification authoring", false),
            ),
            (
                "engine".to_string(),
                role("Clean-room engine implementation", true),
            ),
        ]);

        Self {
            schema_version: SCHEMA_VERSION,
            name: "default".to_string(),
            roles,
            lease_minutes: default_lease_minutes(),
            export: ExportConfig::default(),
        }
    }
}
