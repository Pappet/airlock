//! Project configuration, stored hub-side as `project.json`.

use crate::core::check_task_prefix;
use anyhow::{Result, anyhow, bail};
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleConfig {
    /// Human-readable description of the role's purpose
    pub description: String,
    /// Whether sessions bound to this role get the restricted clean-room tool set
    #[serde(default)]
    pub clean_room: bool,
    /// Short rules shown to sessions of this role in their briefing
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Files every session of this role reads first
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
    /// Task id prefix (`E` gives `E-1`, `E-2`, …) [default: the role name's
    /// first letter or digit, upper-cased]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
}

impl RoleConfig {
    /// The configured prefix, else one derived from the role name.
    fn effective_prefix(&self, name: &str) -> Option<String> {
        self.prefix.clone().or_else(|| {
            name.chars()
                .find(char::is_ascii_alphanumeric)
                .map(|c| c.to_ascii_uppercase().to_string())
        })
    }
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

impl ProjectConfig {
    /// The id prefix for new tasks of `role`. Fails on an unknown role or
    /// while any two roles share a prefix (see `check_prefixes`).
    pub fn task_prefix(&self, role: &str) -> Result<String> {
        let config = self.roles.get(role).ok_or_else(|| {
            let names: Vec<&str> = self.roles.keys().map(String::as_str).collect();
            anyhow!("unknown role `{role}`; defined roles: {}", names.join(", "))
        })?;
        self.check_prefixes()?;
        Ok(config.effective_prefix(role).expect("checked above"))
    }

    /// Every role has a valid task prefix, and no two roles share one.
    pub fn check_prefixes(&self) -> Result<()> {
        let mut owners: BTreeMap<String, &str> = BTreeMap::new();
        for (name, role) in &self.roles {
            let Some(prefix) = role.effective_prefix(name) else {
                bail!(
                    "role `{name}` needs a task prefix: set one with `airlock role set {name} --prefix <P>`"
                );
            };
            check_task_prefix(&prefix).map_err(|e| anyhow!("role `{name}`: {e}"))?;
            if let Some(other) = owners.insert(prefix.clone(), name) {
                bail!(
                    "roles `{other}` and `{name}` share the task prefix `{prefix}`; \
                     give one its own with `airlock role set <role> --prefix <P>`"
                );
            }
        }
        Ok(())
    }
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
            ..RoleConfig::default()
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
