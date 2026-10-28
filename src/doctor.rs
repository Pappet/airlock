//! `airlock doctor`: checks the installation, worktrees, bindings, and harness
//! configurations (Spec 06 §5). Read-only.

use crate::app::Output;
use crate::git;
use crate::hub::{Binding, Hub, MARKER_FILE, Marker, Project, binding_key};
use crate::layout::{LAYOUT_FILE, Layout};
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Level {
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Serialize)]
struct Finding {
    level: Level,
    check: &'static str,
    message: String,
}

#[derive(Default)]
struct Report(Vec<Finding>);

impl Report {
    fn add(&mut self, level: Level, check: &'static str, message: impl Into<String>) {
        self.0.push(Finding {
            level,
            check,
            message: message.into(),
        });
    }
}

pub(crate) fn run(hub: &Hub, cwd: &Path, selector: Option<&str>, out: &Output) -> Result<()> {
    let mut report = Report::default();
    check_binary(&mut report);
    report.add(Level::Ok, "hub", format!("hub at {}", hub.root().display()));
    match hub.resolve(selector, cwd) {
        Ok(project) => check_project(&mut report, &project)?,
        Err(e) => report.add(Level::Warn, "project", format!("no project here: {e:#}")),
    }

    out.print(&report.0, || {
        report
            .0
            .iter()
            .map(|f| {
                let tag = match f.level {
                    Level::Ok => "ok   ",
                    Level::Warn => "warn ",
                    Level::Error => "error",
                };
                format!("{tag} {:<10} {}", f.check, f.message)
            })
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    let errors = report.0.iter().filter(|f| f.level == Level::Error).count();
    if errors > 0 {
        bail!("doctor found {errors} error(s)");
    }
    Ok(())
}

fn check_binary(report: &mut Report) {
    let current = std::env::current_exe().and_then(std::fs::canonicalize).ok();
    let name = if cfg!(windows) {
        "airlock.exe"
    } else {
        "airlock"
    };
    let in_path = find_in_path(name);
    match (&in_path, &current) {
        (None, _) => report.add(
            Level::Warn,
            "binary",
            "`airlock` is not in PATH; harness configurations that call it by name will fail",
        ),
        (Some(found), Some(current)) if std::fs::canonicalize(found).ok().as_ref() == Some(current) => {
            report.add(Level::Ok, "binary", format!("{} (running)", found.display()))
        }
        (Some(found), Some(current)) if same_content(found, current) => report.add(
            Level::Ok,
            "binary",
            format!("{} (identical to the running {})", found.display(), current.display()),
        ),
        (Some(found), Some(current)) => report.add(
            Level::Warn,
            "binary",
            format!(
                "`airlock` in PATH is {}, but the running binary is {} and differs; one installation is stale",
                found.display(),
                current.display()
            ),
        ),
        (Some(found), None) => report.add(Level::Ok, "binary", found.display().to_string()),
    }

    // Other common install locations that differ from the one in PATH.
    if let (Some(home), Some(reference)) = (dirs::home_dir(), in_path.or(current)) {
        for dir in [".cargo/bin", ".local/bin"] {
            let other = home.join(dir).join(name);
            let same_file =
                std::fs::canonicalize(&other).ok() == std::fs::canonicalize(&reference).ok();
            if other.is_file() && !same_file && !same_content(&other, &reference) {
                report.add(
                    Level::Warn,
                    "binary",
                    format!(
                        "{} differs from {}; update or remove the stale copy",
                        other.display(),
                        reference.display()
                    ),
                );
            }
        }
    }
}

fn check_project(report: &mut Report, project: &Project) -> Result<()> {
    report.add(
        Level::Ok,
        "project",
        format!("{} ({})", project.config.name, project.id),
    );
    let bindings = crate::store::Store::open(&project.database)?.bindings()?;

    let Some(root) = &project.root else {
        report.add(
            Level::Warn,
            "project",
            "selected with --project only; run doctor inside a worktree for more checks",
        );
        check_bindings(report, project, &bindings);
        return Ok(());
    };

    match git::query(root, &["ls-files", "--error-unmatch", "--", MARKER_FILE]) {
        Some(Ok(_)) => report.add(Level::Ok, "marker", format!("{MARKER_FILE} is committed")),
        Some(Err(_)) => report.add(
            Level::Warn,
            "marker",
            format!("{MARKER_FILE} is not committed; other worktrees and clones will not find the project"),
        ),
        None => {}
    }

    let worktrees = git::worktrees(root).unwrap_or_else(|| vec![root.clone()]);
    for worktree in &worktrees {
        check_worktree(report, project, &bindings, worktree);
    }
    check_bindings(report, project, &bindings);
    check_layout(report, worktrees.first().unwrap_or(root), &bindings)?;
    let main = worktrees.first().unwrap_or(root);
    let layout_roles: BTreeMap<String, Vec<String>> = Layout::read(main)?
        .map(|layout| {
            layout
                .worktrees
                .iter()
                .filter_map(|spec| {
                    Some((
                        binding_key(&Layout::resolve(main, spec)).ok()?,
                        spec.roles.clone(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    check_harnesses(report, project, &bindings, &layout_roles, &worktrees);
    Ok(())
}

fn check_worktree(
    report: &mut Report,
    project: &Project,
    bindings: &BTreeMap<String, Binding>,
    worktree: &Path,
) {
    let shown = worktree.display();
    match Marker::read(worktree) {
        Ok(Some(marker)) if marker.project_id == project.id => {
            let binding = binding_key(worktree).ok().and_then(|k| bindings.get(&k));
            match binding {
                Some(b) => report.add(
                    Level::Ok,
                    "worktree",
                    format!(
                        "{shown}: role {}, session {}{}",
                        b.role,
                        b.session,
                        if b.locked { " (locked)" } else { "" }
                    ),
                ),
                None => report.add(
                    Level::Warn,
                    "worktree",
                    format!("{shown}: no binding; run `airlock use <role>` there"),
                ),
            }
        }
        Ok(Some(marker)) => report.add(
            Level::Warn,
            "worktree",
            format!(
                "{shown}: marker belongs to another project ({})",
                marker.project_id
            ),
        ),
        Ok(None) => report.add(
            Level::Warn,
            "worktree",
            format!("{shown}: no {MARKER_FILE} marker; merge the branch that holds it or copy it"),
        ),
        Err(e) => report.add(Level::Error, "worktree", format!("{shown}: {e:#}")),
    }
}

fn check_bindings(report: &mut Report, project: &Project, bindings: &BTreeMap<String, Binding>) {
    let mut sessions: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (path, binding) in bindings {
        if !project.config.roles.contains_key(&binding.role) {
            report.add(
                Level::Error,
                "binding",
                format!("{path}: role `{}` no longer exists", binding.role),
            );
        }
        if !Path::new(path).join(MARKER_FILE).is_file() {
            report.add(
                Level::Warn,
                "binding",
                format!("{path}: directory or marker is gone; remove with `airlock -C <dir> use --clear --force` or ignore"),
            );
        }
        sessions.entry(&binding.session).or_default().push(path);
    }
    for (session, paths) in sessions.into_iter().filter(|(_, p)| p.len() > 1) {
        report.add(
            Level::Warn,
            "binding",
            format!(
                "session {session} is bound in several worktrees: {}",
                paths.join(", ")
            ),
        );
    }
}

/// Compares bindings with the committed layout of the main checkout.
fn check_layout(
    report: &mut Report,
    main: &Path,
    bindings: &BTreeMap<String, Binding>,
) -> Result<()> {
    let Some(layout) = Layout::read(main)? else {
        return Ok(());
    };
    for spec in &layout.worktrees {
        let path = Layout::resolve(main, spec);
        let Some(binding) = binding_key(&path).ok().and_then(|k| bindings.get(&k)) else {
            continue;
        };
        if !spec.roles.contains(&binding.role) {
            report.add(
                Level::Warn,
                "layout",
                format!(
                    "{}: bound to role {}, but {LAYOUT_FILE} lists {}",
                    path.display(),
                    binding.role,
                    spec.roles.join(", ")
                ),
            );
        } else if spec.lock && !binding.locked {
            report.add(
                Level::Warn,
                "layout",
                format!("{}: {LAYOUT_FILE} requires a lock, but the binding is not locked; run `airlock init`", path.display()),
            );
        }
    }
    Ok(())
}

/// One airlock MCP server entry found in a harness configuration.
struct Entry {
    label: String,
    /// The worktree a configuration applies to (its directory or the client's `cwd`)
    worktree: Option<PathBuf>,
    /// Whether the configuration file itself belongs to one worktree (e.g. `.mcp.json`)
    per_directory: bool,
    command: String,
    args: Vec<String>,
}

fn check_harnesses(
    report: &mut Report,
    project: &Project,
    bindings: &BTreeMap<String, Binding>,
    layout_roles: &BTreeMap<String, Vec<String>>,
    worktrees: &[PathBuf],
) {
    let home = dirs::home_dir();
    let mut entries = Vec::new();
    for worktree in worktrees {
        let path = worktree.join(".mcp.json");
        collect(
            &mut entries,
            &path,
            Some(worktree),
            &format!("Claude Code {}", path.display()),
            |v| v.get("mcpServers"),
        );
    }
    if let Some(home) = &home {
        let claude = home.join(".claude.json");
        for worktree in worktrees {
            let key = worktree.to_string_lossy().into_owned();
            collect(
                &mut entries,
                &claude,
                Some(worktree),
                &format!("Claude Code local {key}"),
                |v| v.get("projects")?.get(&key)?.get("mcpServers"),
            );
        }
        let antigravity = home.join(".gemini/antigravity/mcp_config.json");
        collect(
            &mut entries,
            &antigravity,
            None,
            &format!("Antigravity {}", antigravity.display()),
            |v| v.get("mcpServers"),
        );
        if let Ok(profiles) = std::fs::read_dir(home.join(".dsh/profiles")) {
            for profile in profiles.flatten() {
                let patch = profile.path().join("cordis.patch.yml");
                if std::fs::read_to_string(&patch).is_ok_and(|t| t.contains("airlock")) {
                    let name = profile.file_name().to_string_lossy().into_owned();
                    report.add(
                        Level::Ok,
                        "harness",
                        format!("DSH profile {name} mentions airlock (not parsed); inspect with `dsh --profile {name} --dump-config`"),
                    );
                }
            }
        }
    }

    let mut sessions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in &entries {
        // Accept both `--role lead` and `--role=lead`.
        let args: Vec<String> = entry
            .args
            .iter()
            .flat_map(
                |a| match a.split_once('=').filter(|(k, _)| k.starts_with("--")) {
                    Some((k, v)) => vec![k.to_string(), v.to_string()],
                    None => vec![a.clone()],
                },
            )
            .collect();
        let flag = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
        };
        let mut problems = Vec::new();

        let command = Path::new(&entry.command);
        if command.is_absolute() {
            if !command.is_file() {
                problems.push(format!("command {} not found", entry.command));
            }
        } else {
            // Clients started from the desktop often have a different PATH than the shell.
            let suggestion = find_in_path(&entry.command)
                .or_else(|| std::env::current_exe().ok())
                .and_then(|p| std::fs::canonicalize(p).ok())
                .map(|p| format!("; use the absolute path {}", p.display()))
                .unwrap_or_default();
            problems.push(format!(
                "command `{}` is looked up in the client's PATH, which can differ from your shell's{suggestion}",
                entry.command
            ));
        }
        let target = flag("-C")
            .map(PathBuf::from)
            .or_else(|| entry.worktree.clone());
        if target.is_none() && flag("--project").is_none() {
            problems.push(
                "no -C or --project: works only if the client starts it inside the project"
                    .to_string(),
            );
        }
        let binding = target
            .as_deref()
            .and_then(|t| binding_key(t).ok())
            .and_then(|k| bindings.get(&k));
        if let (Some(role), Some(b)) = (flag("--role"), binding) {
            if b.locked && *role != b.role {
                problems.push(format!(
                    "--role {role} contradicts the locked binding to {}",
                    b.role
                ));
            }
        }
        // A per-directory configuration that overrides the binding splits CLI and MCP identities,
        // unless it runs another role the layout places in this shared worktree (e.g. lead next to spec).
        let key = target.as_deref().and_then(|t| binding_key(t).ok());
        let second_role = flag("--role").is_some_and(|r| {
            binding.is_some_and(|b| *r != b.role)
                && key
                    .as_ref()
                    .and_then(|k| layout_roles.get(k))
                    .is_some_and(|roles| roles.contains(r))
        });
        if let (true, Some(b), false) = (entry.per_directory, binding, second_role) {
            if let Some(session) = flag("--session").filter(|s| **s != b.session) {
                problems.push(format!(
                    "--session {session} differs from the worktree's bound session {}; CLI and MCP would act as \
                     different sessions — drop --session and --role to use the binding",
                    b.session
                ));
            }
            if let Some(role) = flag("--role").filter(|r| **r != b.role && !b.locked) {
                problems.push(format!(
                    "--role {role} differs from the worktree's bound role {}; CLI and MCP would act as different roles",
                    b.role
                ));
            }
        }
        if let Some(role) = flag("--role").filter(|r| !project.config.roles.contains_key(*r)) {
            problems.push(format!("unknown role {role}"));
        }
        let session = flag("--session")
            .cloned()
            .or_else(|| binding.map(|b| b.session.clone()));
        if let Some(session) = session {
            sessions
                .entry(session)
                .or_default()
                .push(entry.label.clone());
        }

        if problems.is_empty() {
            report.add(
                Level::Ok,
                "harness",
                format!(
                    "{}: {} {}",
                    entry.label,
                    entry.command,
                    entry.args.join(" ")
                ),
            );
        } else {
            report.add(
                Level::Warn,
                "harness",
                format!("{}: {}", entry.label, problems.join("; ")),
            );
        }
    }
    for (session, labels) in sessions.into_iter().filter(|(_, l)| l.len() > 1) {
        report.add(
            Level::Warn,
            "harness",
            format!(
                "session {session} is used by several configurations ({}); concurrent agents would share leases",
                labels.join(", ")
            ),
        );
    }
}

/// Adds the airlock servers from one JSON configuration file.
fn collect(
    entries: &mut Vec<Entry>,
    path: &Path,
    worktree: Option<&Path>,
    label: &str,
    servers: impl for<'a> Fn(&'a Value) -> Option<&'a Value>,
) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return;
    };
    let Some(Value::Object(servers)) = servers(&json) else {
        return;
    };
    for (name, server) in servers {
        let Some(command) = server.get("command").and_then(Value::as_str) else {
            continue;
        };
        let is_airlock = Path::new(command)
            .file_stem()
            .is_some_and(|s| s == "airlock");
        if !is_airlock {
            continue;
        }
        let args = server
            .get("args")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        // A client-side working directory counts like -C.
        let worktree = server
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| worktree.map(Path::to_path_buf));
        entries.push(Entry {
            label: format!("{label} [{name}]"),
            per_directory: worktree.is_some() && server.get("cwd").is_none(),
            worktree,
            command: command.to_string(),
            args,
        });
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn same_content(a: &Path, b: &Path) -> bool {
    let hash = |p: &Path| std::fs::read(p).ok().map(Sha256::digest);
    matches!((hash(a), hash(b)), (Some(x), Some(y)) if x == y)
}
