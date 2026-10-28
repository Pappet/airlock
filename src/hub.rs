//! Hub directory, project registry, and `.airlock` marker resolution (Spec 01 §3.1, Spec 02 §2.2).

use crate::config::ProjectConfig;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// File name of the project marker committed to repositories.
pub const MARKER_FILE: &str = ".airlock";

const PROJECTS_DIR: &str = "projects";
const CONFIG_FILE: &str = "project.json";
const DATABASE_FILE: &str = "airlock.db";

/// Contents of the `.airlock` marker file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Marker {
    pub project_id: String,
}

impl Marker {
    /// Reads the marker in `dir`, if present.
    pub fn read(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(MARKER_FILE);
        if !path.is_file() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let marker =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(marker))
    }

    /// Writes the marker into `dir`.
    pub fn write(&self, dir: &Path) -> Result<()> {
        let mut json = serde_json::to_string_pretty(self)?;
        json.push('\n');
        let path = dir.join(MARKER_FILE);
        fs::write(&path, json).with_context(|| format!("writing {}", path.display()))
    }

    /// Finds the nearest marker at or above `start`; returns its directory and contents.
    pub fn find(start: &Path) -> Result<Option<(PathBuf, Self)>> {
        for dir in start.ancestors() {
            if let Some(marker) = Self::read(dir)? {
                return Ok(Some((dir.to_path_buf(), marker)));
            }
        }
        Ok(None)
    }
}

/// The per-user hub directory holding all project stores.
#[derive(Debug, Clone)]
pub struct Hub {
    root: PathBuf,
}

/// A project resolved for one invocation.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub config: ProjectConfig,
    /// Directory of the marker that selected the project, if any
    pub root: Option<PathBuf>,
    /// Path of the SQLite store
    pub database: PathBuf,
}

impl Hub {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `$AIRLOCK_HOME`, else the platform state directory joined with `airlock`.
    pub fn locate() -> Result<Self> {
        if let Some(home) = std::env::var_os("AIRLOCK_HOME").filter(|v| !v.is_empty()) {
            return Ok(Self::new(home));
        }
        let base = dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .context("cannot determine a state directory; set AIRLOCK_HOME")?;
        Ok(Self::new(base.join("airlock")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn project_dir(&self, id: &str) -> PathBuf {
        self.root.join(PROJECTS_DIR).join(id)
    }

    pub fn database_path(&self, id: &str) -> PathBuf {
        self.project_dir(id).join(DATABASE_FILE)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.project_dir(id).join(CONFIG_FILE).is_file()
    }

    pub fn load_config(&self, id: &str) -> Result<ProjectConfig> {
        let path = self.project_dir(id).join(CONFIG_FILE);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Writes `project.json` atomically (temp file + rename).
    pub fn save_config(&self, id: &str, config: &ProjectConfig) -> Result<()> {
        let dir = self.project_dir(id);
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let mut json = serde_json::to_string_pretty(config)?;
        json.push('\n');
        let tmp = dir.join(format!("{CONFIG_FILE}.tmp"));
        fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, dir.join(CONFIG_FILE)).context("replacing project.json")?;
        Ok(())
    }

    /// All registered projects as `(id, config)`, sorted by id.
    pub fn projects(&self) -> Result<Vec<(String, ProjectConfig)>> {
        let dir = self.root.join(PROJECTS_DIR);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut projects = Vec::new();
        for entry in fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))? {
            let Ok(id) = entry?.file_name().into_string() else {
                continue;
            };
            if self.contains(&id) {
                let config = self.load_config(&id)?;
                projects.push((id, config));
            }
        }
        projects.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(projects)
    }

    /// Looks up a project by uuid or by unique name.
    pub fn find_project(&self, id_or_name: &str) -> Result<String> {
        if self.contains(id_or_name) {
            return Ok(id_or_name.to_string());
        }
        let matches: Vec<String> = self
            .projects()?
            .into_iter()
            .filter(|(_, config)| config.name == id_or_name)
            .map(|(id, _)| id)
            .collect();
        match matches.as_slice() {
            [id] => Ok(id.clone()),
            [] => bail!(
                "no project with id or name `{id_or_name}` in hub {}",
                self.root.display()
            ),
            _ => bail!(
                "project name `{id_or_name}` is ambiguous; use the project id ({})",
                matches.join(", ")
            ),
        }
    }

    /// Resolves the project for an invocation: explicit selector first, then the nearest marker above `cwd`.
    pub fn resolve(&self, selector: Option<&str>, cwd: &Path) -> Result<Project> {
        let found = Marker::find(cwd)?;
        let (id, root) = match selector {
            Some(selector) => {
                let id = self.find_project(selector)?;
                let root = found
                    .filter(|(_, m)| m.project_id == id)
                    .map(|(dir, _)| dir);
                (id, root)
            }
            None => {
                let Some((dir, marker)) = found else {
                    bail!(
                        "not inside an Airlock project (no {MARKER_FILE} found); run `airlock init` or pass --project"
                    );
                };
                if !self.contains(&marker.project_id) {
                    bail!(
                        "project {} from {} is not registered in this hub; run `airlock init` there to adopt it",
                        marker.project_id,
                        dir.join(MARKER_FILE).display()
                    );
                }
                (marker.project_id, Some(dir))
            }
        };
        Ok(Project {
            config: self.load_config(&id)?,
            database: self.database_path(&id),
            id,
            root,
        })
    }
}
