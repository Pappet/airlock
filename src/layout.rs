//! Project layout (`airlock.toml`) and init templates (Spec 06 §7).

use crate::config::{ProjectConfig, RoleConfig};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// File name of the committed layout, next to the `.airlock` marker of the main checkout.
pub const LAYOUT_FILE: &str = "airlock.toml";

/// Which worktree hosts which roles. Roles themselves are defined in the
/// project configuration, not here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Layout {
    #[serde(rename = "worktree")]
    pub worktrees: Vec<WorktreeSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeSpec {
    /// Path relative to the main checkout; `.` is the main checkout itself
    pub path: String,
    /// Branch checked out in the worktree [default: the directory name]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Roles that work here; the first one is bound
    pub roles: Vec<String>,
    /// Lock the binding (clean-room worktrees)
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lock: bool,
}

impl Layout {
    pub fn read(main: &Path) -> Result<Option<Self>> {
        let path = main.join(LAYOUT_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let layout: Self =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        layout
            .validate()
            .with_context(|| format!("checking {}", path.display()))?;
        Ok(Some(layout))
    }

    pub fn write(&self, main: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self)?;
        let text = format!(
            "# Airlock project layout: which worktree hosts which roles.\n\
             # `airlock init` creates missing worktrees and bindings from it on any machine.\n\
             # Roles are defined with `airlock role`; paths are relative to the main checkout.\n\n{body}"
        );
        let path = main.join(LAYOUT_FILE);
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.worktrees.iter().any(|w| w.path == "."),
            "the layout needs an entry with path \".\""
        );
        for w in &self.worktrees {
            ensure!(!w.roles.is_empty(), "worktree `{}` lists no roles", w.path);
            ensure!(
                !Path::new(&w.path).is_absolute(),
                "worktree path `{}` must be relative to the main checkout",
                w.path
            );
        }
        Ok(())
    }

    /// The absolute path of a worktree entry, resolved against the main checkout.
    pub fn resolve(main: &Path, spec: &WorktreeSpec) -> PathBuf {
        if spec.path == "." {
            return main.to_path_buf();
        }
        // Resolve `.` and `..` lexically; the worktree may not exist yet.
        let mut out = PathBuf::new();
        for component in main.join(&spec.path).components() {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    out.pop();
                }
                other => out.push(other),
            }
        }
        out
    }

    /// Roles referenced by the layout.
    pub fn roles(&self) -> impl Iterator<Item = &str> {
        self.worktrees
            .iter()
            .flat_map(|w| w.roles.iter().map(String::as_str))
    }
}

/// A starting point for `airlock init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Template {
    /// Roles lead, spec, engine; no worktrees
    Default,
    /// One role `dev`, no worktrees, bound
    Single,
    /// lead + spec in the main checkout, engine in a locked clean-room worktree
    CleanRoom,
    /// lead in the main checkout, one `dev` worktree per agent
    Parallel,
}

impl Template {
    /// The name as typed on the command line, e.g. `clean-room`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Single => "single",
            Self::CleanRoom => "clean-room",
            Self::Parallel => "parallel",
        }
    }

    pub fn describe(self, dir: &str, agents: u32) -> String {
        match self {
            Self::Default => "roles lead, spec, engine; no worktrees".to_string(),
            Self::Single => "one role (dev) in this checkout".to_string(),
            Self::CleanRoom => {
                format!("lead + spec here, engine in ../{dir}-engine (clean-room, locked)")
            }
            Self::Parallel => format!("lead here, {agents} dev worktree(s) ../{dir}-dev-<n>"),
        }
    }

    /// Role definitions the template creates.
    pub fn roles(self) -> BTreeMap<String, RoleConfig> {
        let role = |description: &str| RoleConfig {
            description: description.to_string(),
            ..RoleConfig::default()
        };
        match self {
            Self::Default => ProjectConfig::default().roles,
            Self::Single => BTreeMap::from([("dev".to_string(), role("Development"))]),
            Self::CleanRoom => {
                let mut roles = ProjectConfig::default().roles;
                if let Some(engine) = roles.get_mut("engine") {
                    engine.instructions = Some(
                        "Implement only from the specifications and documents listed in your work packages. \
                         Never read analysis material (decompiled output, disassembly, the main checkout). \
                         If a specification is unclear, ask a question instead of guessing."
                            .to_string(),
                    );
                }
                roles
            }
            Self::Parallel => BTreeMap::from([
                (
                    "lead".to_string(),
                    role("Planning, task breakdown, and review"),
                ),
                ("dev".to_string(), role("Implementation of work packages")),
            ]),
        }
    }

    /// The layout the template creates; `None` for templates without worktrees or bindings.
    pub fn layout(self, dir: &str, agents: u32) -> Option<Layout> {
        let here = |roles: &[&str]| WorktreeSpec {
            path: ".".to_string(),
            branch: None,
            roles: roles.iter().map(|r| r.to_string()).collect(),
            lock: false,
        };
        let worktrees = match self {
            Self::Default => return None,
            Self::Single => vec![here(&["dev"])],
            Self::CleanRoom => vec![
                here(&["lead", "spec"]),
                WorktreeSpec {
                    path: format!("../{dir}-engine"),
                    branch: Some("engine".to_string()),
                    roles: vec!["engine".to_string()],
                    lock: true,
                },
            ],
            Self::Parallel => std::iter::once(here(&["lead"]))
                .chain((1..=agents).map(|n| WorktreeSpec {
                    path: format!("../{dir}-dev-{n}"),
                    branch: Some(format!("dev-{n}")),
                    roles: vec!["dev".to_string()],
                    lock: false,
                }))
                .collect(),
        };
        Some(Layout { worktrees })
    }
}
