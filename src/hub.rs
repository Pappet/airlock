//! Hub directory, project registry, and `.airlock` marker resolution (Spec 01 §3.1, Spec 02 §2.2).

use crate::config::ProjectConfig;
use crate::store::Store;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// File name of the project marker committed to repositories.
pub const MARKER_FILE: &str = ".airlock";

const PROJECTS_DIR: &str = "projects";
const CONFIG_FILE: &str = "project.json";
const DATABASE_FILE: &str = "airlock.db";

/// A worktree's role and session (Spec 06 §2), keyed by canonical project root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Binding {
    pub role: String,
    pub session: String,
    /// Refuse role changes without `--force`
    #[serde(default)]
    pub locked: bool,
}

/// All bindings of one project, keyed by canonical root path.
pub type Bindings = BTreeMap<String, Binding>;

/// Whether `id` is a project id: a UUID in canonical lowercase hyphenated form.
pub fn is_project_id(id: &str) -> bool {
    uuid::Uuid::try_parse(id).is_ok_and(|uuid| uuid.hyphenated().to_string() == id)
}

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
        let marker: Self =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        // The marker comes from the repository, so its id is untrusted input
        // that ends up in hub paths.
        ensure!(
            is_project_id(&marker.project_id),
            "invalid project id `{}` in {}: expected a lowercase hyphenated UUID",
            marker.project_id,
            path.display()
        );
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
    /// Binding of `root`, if the worktree is bound to a role
    pub binding: Option<Binding>,
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

    /// The project's directory; rejects anything but a canonical UUID so ids can never escape the hub.
    fn project_dir(&self, id: &str) -> Result<PathBuf> {
        ensure!(
            is_project_id(id),
            "invalid project id `{id}`: expected a lowercase hyphenated UUID"
        );
        Ok(self.root.join(PROJECTS_DIR).join(id))
    }

    pub fn database_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self.project_dir(id)?.join(DATABASE_FILE))
    }

    pub fn contains(&self, id: &str) -> bool {
        self.project_dir(id)
            .is_ok_and(|dir| dir.join(CONFIG_FILE).is_file())
    }

    pub fn load_config(&self, id: &str) -> Result<ProjectConfig> {
        let path = self.project_dir(id)?.join(CONFIG_FILE);
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
    }

    /// Writes `project.json` atomically (temp file + rename).
    pub fn save_config(&self, id: &str, config: &ProjectConfig) -> Result<()> {
        let dir = self.project_dir(id)?;
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
        let database = self.database_path(&id)?;
        let binding = match &root {
            Some(root) => Store::open(&database)?.binding(&binding_key(root)?)?,
            None => None,
        };
        Ok(Project {
            config: self.load_config(&id)?,
            database,
            id,
            root,
            binding,
        })
    }
}

/// The key of a project root in the bindings table: its canonical absolute path.
pub fn binding_key(root: &Path) -> Result<String> {
    let path = fs::canonicalize(root).with_context(|| format!("resolving {}", root.display()))?;
    path.into_os_string()
        .into_string()
        .map_err(|p| anyhow::anyhow!("path {} is not valid UTF-8", p.to_string_lossy()))
}
