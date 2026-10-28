//! `airlock sync` and `airlock import` (Spec 05 §3).

use crate::config::ExportConfig;
use crate::core::check_task_id;
use crate::hub::{Hub, Project};
use crate::snapshot::{FORMAT_VERSION, Snapshot};
use crate::store::Store;
use crate::views;
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

/// Outcome of one `sync` run, as paths relative to the project root.
#[derive(Debug, Default, serde::Serialize)]
pub struct SyncReport {
    pub written: Vec<String>,
    pub unchanged: Vec<String>,
}

/// Exports the snapshot and the enabled views into `root`.
///
/// Refuses to overwrite a snapshot this hub never wrote or imported unless
/// `force` is set (foreign-change guard).
pub fn sync(
    store: &mut Store,
    project: &Project,
    root: &Path,
    force: bool,
    now: i64,
) -> Result<SyncReport> {
    let snapshot = Snapshot::new(
        &project.id,
        &project.config,
        &store.tasks(now)?,
        &store.questions()?,
    );
    let json = snapshot.to_canonical_json()?;
    let export = &project.config.export;
    let state_path = export_path(root, &export.state_file)?;

    match fs::read(&state_path) {
        Ok(existing) if existing == json.as_bytes() => {}
        Ok(existing) => {
            if !force && !store.is_known_export(&sha256_hex(&existing))? {
                bail!(
                    "{} was not written by this hub (another machine or a manual edit). \
                     Inspect it, then run `airlock import` to adopt it or `airlock sync --force` to replace it",
                    state_path.display()
                );
            }
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("reading {}", state_path.display())),
    }

    let mut report = SyncReport::default();
    for (rel, content) in artifacts(&snapshot, export, json) {
        let path = export_path(root, &rel)?;
        if rel_is(&rel, &export.state_file) {
            store.record_export(&sha256_hex(content.as_bytes()), now)?;
        }
        if fs::read(&path).is_ok_and(|existing| existing == content.as_bytes()) {
            report.unchanged.push(rel);
        } else {
            write_atomic(&path, &content)?;
            report.written.push(rel);
        }
    }
    Ok(report)
}

/// A snapshot read from disk, with the hash of its exact bytes.
pub struct LoadedSnapshot {
    pub snapshot: Snapshot,
    pub sha256: String,
}

pub fn load_snapshot(path: &Path) -> Result<LoadedSnapshot> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let snapshot =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    Ok(LoadedSnapshot {
        snapshot,
        sha256: sha256_hex(&bytes),
    })
}

/// Checks a snapshot before it replaces a project's state (Spec 05 §3.4).
pub fn check_import(snapshot: &Snapshot, project_id: &str) -> Result<()> {
    ensure!(
        snapshot.format_version == FORMAT_VERSION,
        "snapshot format version {} is not supported (expected {FORMAT_VERSION})",
        snapshot.format_version
    );
    ensure!(
        snapshot.project_id == project_id,
        "snapshot belongs to project {}, not {project_id}",
        snapshot.project_id
    );
    for task in &snapshot.tasks {
        check_task_id(&task.id).map_err(anyhow::Error::msg)?;
        ensure!(
            snapshot.roles.contains_key(&task.role),
            "task `{}` has role `{}`, which the snapshot does not define",
            task.id,
            task.role
        );
    }
    snapshot.validate()?;
    let mut ids = HashSet::new();
    for question in &snapshot.questions {
        ensure!(question.id > 0, "question ids start at 1");
        ensure!(
            ids.insert(question.id),
            "duplicate question id #{}",
            question.id
        );
    }
    Ok(())
}

/// Replaces the project's tasks, questions, and roles with the snapshot and
/// records its hash as known to this hub.
pub fn import(
    store: &mut Store,
    hub: &Hub,
    project: &Project,
    loaded: &LoadedSnapshot,
    now: i64,
) -> Result<()> {
    check_import(&loaded.snapshot, &project.id)?;
    store.replace_all(&loaded.snapshot, now)?;
    let mut config = project.config.clone();
    config.roles = loaded.snapshot.roles.clone();
    hub.save_config(&project.id, &config)?;
    store.record_export(&loaded.sha256, now)
}

/// The configured export path of the snapshot, inside `root`.
pub fn state_path(root: &Path, export: &ExportConfig) -> Result<PathBuf> {
    export_path(root, &export.state_file)
}

fn artifacts(snapshot: &Snapshot, export: &ExportConfig, json: String) -> Vec<(String, String)> {
    let mut files = vec![(export.state_file.clone(), json)];
    if let Some(plan) = &export.plan_file {
        files.push((plan.clone(), views::plan(snapshot)));
    }
    if let Some(questions) = &export.questions_file {
        files.push((questions.clone(), views::questions(snapshot)));
    }
    files
}

/// Joins a configured relative path onto `root`, refusing absolute paths and `..`.
fn export_path(root: &Path, rel: &str) -> Result<PathBuf> {
    let path = Path::new(rel);
    ensure!(
        !rel.is_empty()
            && path
                .components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "export path `{rel}` must be relative to the project root and stay inside it"
    );
    Ok(root.join(path))
}

fn rel_is(a: &str, b: &str) -> bool {
    Path::new(a) == Path::new(b)
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".airlock-tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
