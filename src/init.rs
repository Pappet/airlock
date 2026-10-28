//! `airlock init`: registers a project and sets up its layout (Spec 02 §3.1, Spec 06 §7).
//!
//! Init is idempotent. It registers a new project or adopts an existing
//! marker, optionally applies a template, and then makes the machine match
//! `airlock.toml`: missing worktrees are created and unbound ones bound.

use crate::app::{Output, confirm};
use crate::config::{ProjectConfig, RoleConfig};
use crate::git;
use crate::hub::{Binding, Hub, MARKER_FILE, Marker, binding_key};
use crate::layout::{LAYOUT_FILE, Layout, Template};
use crate::store::Store;
use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::Path;

pub(crate) struct Options {
    pub name: Option<String>,
    pub template: Option<Template>,
    pub agents: u32,
    pub yes: bool,
}

/// One step init took, for the report.
#[derive(Debug, Serialize)]
struct Action {
    action: &'static str,
    path: String,
    detail: String,
}

pub(crate) fn run(hub: &Hub, cwd: &Path, opts: Options, out: &Output) -> Result<()> {
    let marker = Marker::read(cwd)?;
    if marker.is_none() {
        if let Some((dir, _)) = Marker::find(cwd)? {
            bail!(
                "{} is already inside the Airlock project at {}",
                cwd.display(),
                dir.display()
            );
        }
    }
    let known = marker.as_ref().is_some_and(|m| hub.contains(&m.project_id));
    let dir = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .context("cannot derive a name from the directory; pass --name")?
        .to_string();
    let interactive =
        std::io::stdin().is_terminal() && std::io::stderr().is_terminal() && !opts.yes;
    let existing_layout = Layout::read(cwd)?;

    // A brand-new project started on a terminal gets the wizard.
    let (template, agents) = match opts.template {
        // On a terminal, a new project gets the wizard, and so does a known project without a
        // layout (where the default answer leaves it unchanged).
        None if interactive && existing_layout.is_none() && (marker.is_none() || known) => {
            prompt_template(&dir, opts.agents, known)?
        }
        template => (template, opts.agents),
    };
    if let (Some(template), Some(_)) = (template, &existing_layout) {
        ensure!(
            template == Template::Default,
            "{LAYOUT_FILE} already exists; edit it instead of applying a template, then run `airlock init`"
        );
    }
    let new_layout = template
        .and_then(|t| t.layout(&dir, agents))
        .filter(|_| existing_layout.is_none());
    let layout = existing_layout.clone().or_else(|| new_layout.clone());

    let in_git = git::worktrees(cwd).and_then(|w| w.into_iter().next());
    let needs_worktrees = layout
        .as_ref()
        .is_some_and(|l| l.worktrees.iter().any(|w| w.path != "."));
    if needs_worktrees {
        let main = in_git
            .as_ref()
            .context("this layout creates worktrees and needs a git repository; run `git init` and commit once")?;
        ensure!(
            same_dir(main, cwd),
            "run `airlock init` in the main checkout ({}), where {LAYOUT_FILE} lives",
            main.display()
        );
        ensure!(
            git::has_commit(cwd),
            "worktrees need at least one commit; commit once, then run `airlock init`"
        );
    }

    let registers = marker.is_none() || !known;
    // Only a new layout needs a commit: worktrees branch off after it and inherit marker and layout.
    let commits = in_git.is_some() && new_layout.is_some();
    if registers || new_layout.is_some() {
        let plan = plan_text(
            &dir,
            marker.is_none(),
            template,
            agents,
            new_layout.as_ref(),
            commits,
        );
        if interactive {
            ensure!(confirm(&plan)?, "init cancelled");
        } else if new_layout.is_some() && !opts.yes {
            bail!("{plan}\nNo terminal to confirm on; pass --yes");
        }
    }

    let mut actions = Vec::new();
    let roles_for_new = template.unwrap_or(Template::Default).roles();

    // 1. Register or adopt the project.
    let (id, name, created, adopted) = match &marker {
        Some(m) if known => {
            let mut config = hub.load_config(&m.project_id)?;
            if let Some(template) = template {
                let added = add_missing_roles(&mut config.roles, template.roles());
                if !added.is_empty() {
                    hub.save_config(&m.project_id, &config)?;
                    actions.push(act("roles", cwd, format!("added {}", added.join(", "))));
                }
            }
            (m.project_id.clone(), config.name, false, false)
        }
        _ => {
            let name = opts.name.clone().unwrap_or_else(|| dir.clone());
            ensure!(
                hub.projects()?.iter().all(|(_, c)| c.name != name),
                "a project named `{name}` already exists in the hub; choose another with --name"
            );
            let mut roles = roles_for_new;
            // Roles a committed layout refers to exist before `airlock import` brings their definitions.
            if let Some(layout) = &layout {
                for role in layout.roles() {
                    roles
                        .entry(role.to_string())
                        .or_insert_with(RoleConfig::default);
                }
            }
            let adopted = marker.is_some();
            let id = marker.as_ref().map_or_else(
                || uuid::Uuid::new_v4().to_string(),
                |m| m.project_id.clone(),
            );
            hub.save_config(
                &id,
                &ProjectConfig {
                    name: name.clone(),
                    roles,
                    ..ProjectConfig::default()
                },
            )?;
            Store::open(&hub.database_path(&id)?)?;
            if adopted {
                actions.push(act(
                    "registered",
                    cwd,
                    format!("existing project {name} ({id}) in {}", hub.root().display()),
                ));
            } else {
                Marker {
                    project_id: id.clone(),
                }
                .write(cwd)?;
                actions.push(act("registered", cwd, format!("project {name} ({id})")));
            }
            (id, name, !adopted, adopted)
        }
    };

    // 2. Write and commit the layout; worktrees branch off after this commit and inherit both files.
    let mut to_commit = Vec::new();
    if created {
        to_commit.push(MARKER_FILE);
    }
    if let Some(layout) = &new_layout {
        layout.write(cwd)?;
        actions.push(act("wrote", &cwd.join(LAYOUT_FILE), String::new()));
        to_commit.push(LAYOUT_FILE);
    }
    if commits && !to_commit.is_empty() {
        commit(cwd, &to_commit, &mut actions)?;
    } else if created {
        actions.push(act(
            "note",
            cwd,
            format!("commit {MARKER_FILE} to the repository"),
        ));
    }

    // 3. Make worktrees and bindings match the layout.
    if let Some(layout) = &layout {
        let config = hub.load_config(&id)?;
        let mut store = Store::open(&hub.database_path(&id)?)?;
        apply(&mut store, cwd, layout, &config, &mut actions)?;
    }

    let snapshot = cwd.join(&ProjectConfig::default().export.state_file);
    let import_hint = adopted && snapshot.is_file();
    out.print(
        &json!({
            "project_id": id, "name": name, "created": created, "adopted": adopted,
            "template": template.map(Template::name), "actions": actions,
        }),
        || {
            if actions.is_empty() {
                return format!("Already initialized: project {name} ({id})");
            }
            let mut lines: Vec<String> = actions
                .iter()
                .map(|a| format!("{:<10} {}{}", a.action, a.path, if a.detail.is_empty() { String::new() } else { format!("  {}", a.detail) }))
                .collect();
            if import_hint {
                lines.push(format!("A snapshot exists at {}; run `airlock import` to load it", snapshot.display()));
            }
            if layout.is_some() {
                lines.push(
                    "Next: run `airlock setup claude|antigravity|dsh` in each worktree; agents begin with \
                     `session_start` (MCP) or `airlock start`."
                        .to_string(),
                );
            }
            lines.join("\n")
        },
    )
}

/// Creates missing worktrees and binds unbound ones. Never changes existing bindings.
fn apply(
    store: &mut Store,
    main: &Path,
    layout: &Layout,
    config: &ProjectConfig,
    actions: &mut Vec<Action>,
) -> Result<()> {
    let now = crate::app::unix_now();
    for spec in &layout.worktrees {
        let path = Layout::resolve(main, spec);
        if spec.path != "." && !path.exists() {
            let dir = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("worktree");
            let branch = spec.branch.clone().unwrap_or_else(|| dir.to_string());
            let target = path.to_string_lossy();
            // An existing local branch, or one from exactly one remote (fresh clone), is checked out as is.
            if git::branch_exists(main, &branch) || git::remote_branch_exists(main, &branch) {
                git::run(main, &["worktree", "add", "--quiet", &target, &branch])?;
            } else {
                git::run(
                    main,
                    &["worktree", "add", "--quiet", "-b", &branch, &target],
                )?;
            }
            actions.push(act(
                "worktree",
                &path,
                format!("created on branch {branch}"),
            ));
        }
        if spec.path != "." && !is_worktree_of(main, &path) {
            actions.push(act(
                "skipped",
                &path,
                "exists but is not a worktree of this repository; move it away or fix the path in airlock.toml"
                    .to_string(),
            ));
            continue;
        }
        if !path.join(MARKER_FILE).is_file() {
            actions.push(act(
                "skipped",
                &path,
                format!("no {MARKER_FILE} marker; merge the main branch into it, then run `airlock init` again"),
            ));
            continue;
        }
        let key = binding_key(&path)?;
        if let Some(b) = store.binding(&key)? {
            if spec.lock && !b.locked && spec.roles.contains(&b.role) {
                // Tightening only: the layout's lock is added, role and session stay.
                store.bind(
                    &key,
                    &Binding {
                        locked: true,
                        ..b.clone()
                    },
                    false,
                    now,
                )?;
                actions.push(act(
                    "locked",
                    &path,
                    format!(
                        "role {}, session {} (airlock.toml requires a lock)",
                        b.role, b.session
                    ),
                ));
            } else if !spec.roles.contains(&b.role) {
                actions.push(act(
                    "bound",
                    &path,
                    format!(
                        "already: role {}, session {} — airlock.toml lists {}; change with `airlock use`",
                        b.role,
                        b.session,
                        spec.roles.join(", ")
                    ),
                ));
            } else {
                actions.push(act(
                    "bound",
                    &path,
                    format!("already: role {}, session {}", b.role, b.session),
                ));
            }
            continue;
        }
        let role = &spec.roles[0];
        if !config.roles.contains_key(role) {
            actions.push(act(
                "skipped",
                &path,
                format!("role `{role}` is not defined; add it with `airlock role set`"),
            ));
            continue;
        }
        let dir = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("worktree");
        let binding = Binding {
            role: role.clone(),
            session: format!("{role}@{dir}"),
            locked: spec.lock,
        };
        store.bind(&key, &binding, false, now)?;
        actions.push(act(
            "bound",
            &path,
            format!(
                "role {}, session {}{}",
                binding.role,
                binding.session,
                if binding.locked { " (locked)" } else { "" }
            ),
        ));
    }
    Ok(())
}

fn commit(dir: &Path, files: &[&str], actions: &mut Vec<Action>) -> Result<()> {
    let mut add = vec!["add", "--"];
    add.extend_from_slice(files);
    git::run(dir, &add)?;
    let mut status = vec!["status", "--porcelain", "--"];
    status.extend_from_slice(files);
    if git::run(dir, &status)?.trim().is_empty() {
        return Ok(());
    }
    // Commit only these paths, leaving anything else that is staged alone.
    let mut args = vec!["commit", "--quiet", "-m", "chore: set up airlock", "--"];
    args.extend_from_slice(files);
    git::run(dir, &args)?;
    actions.push(act("committed", dir, files.join(", ")));
    Ok(())
}

fn add_missing_roles(
    roles: &mut BTreeMap<String, RoleConfig>,
    template: BTreeMap<String, RoleConfig>,
) -> Vec<String> {
    let mut added = Vec::new();
    for (name, role) in template {
        if !roles.contains_key(&name) {
            roles.insert(name.clone(), role);
            added.push(name);
        }
    }
    added
}

fn plan_text(
    dir: &str,
    new: bool,
    template: Option<Template>,
    agents: u32,
    layout: Option<&Layout>,
    commits: bool,
) -> String {
    let mut lines = Vec::new();
    if new {
        lines.push(format!(
            "Register project {dir} with template {}: {}",
            template.map_or("default", Template::name),
            template.unwrap_or(Template::Default).describe(dir, agents)
        ));
    }
    if let Some(layout) = layout {
        lines.push(format!("Write {LAYOUT_FILE}:"));
        for w in &layout.worktrees {
            let place = if w.path == "." {
                "this checkout".to_string()
            } else {
                w.path.clone()
            };
            lines.push(format!(
                "  {place:<28} roles {}{}{}",
                w.roles.join(", "),
                w.branch
                    .as_ref()
                    .map(|b| format!(", branch {b}"))
                    .unwrap_or_default(),
                if w.lock { ", locked" } else { "" }
            ));
        }
    }
    if commits {
        let files = if new {
            format!("{MARKER_FILE} and {LAYOUT_FILE}")
        } else {
            LAYOUT_FILE.to_string()
        };
        lines.push(format!(
            "Commit {files} (nothing else), then create and bind the worktrees."
        ));
    }
    lines.join("\n")
}

/// Asks for a template; for an existing project the default is to change nothing.
fn prompt_template(dir: &str, agents: u32, existing: bool) -> Result<(Option<Template>, u32)> {
    let mut choices: Vec<(Option<Template>, String)> = Vec::new();
    if existing {
        choices.push((None, "leave the project as it is".to_string()));
    }
    for t in [
        Template::Default,
        Template::Single,
        Template::CleanRoom,
        Template::Parallel,
    ] {
        // In an existing project, `default` would only add roles; offer layouts only.
        if !(existing && t == Template::Default) {
            choices.push((Some(t), t.describe(dir, agents)));
        }
    }
    let mut stderr = std::io::stderr();
    writeln!(
        stderr,
        "{}",
        if existing {
            "Apply a layout template?"
        } else {
            "Template:"
        }
    )?;
    for (i, (t, text)) in choices.iter().enumerate() {
        writeln!(
            stderr,
            "  {}) {:<11} {text}",
            i + 1,
            t.map_or("none", Template::name)
        )?;
    }
    let template = loop {
        let answer = ask(&format!("Choose [1-{}, default 1]: ", choices.len()))?;
        match answer
            .parse::<usize>()
            .ok()
            .or((answer.is_empty()).then_some(1))
        {
            Some(n) if (1..=choices.len()).contains(&n) => break choices[n - 1].0,
            _ => writeln!(stderr, "Please enter a number from 1 to {}.", choices.len())?,
        }
    };
    let agents = if template == Some(Template::Parallel) {
        loop {
            match ask(&format!("Number of dev worktrees [{agents}]: "))?.as_str() {
                "" => break agents,
                n => match n.parse().ok().filter(|n| (1..=16).contains(n)) {
                    Some(n) => break n,
                    None => writeln!(stderr, "Please enter a number from 1 to 16.")?,
                },
            }
        }
    } else {
        agents
    };
    Ok((template, agents))
}

fn ask(prompt: &str) -> Result<String> {
    eprint!("{prompt}");
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().to_string())
}

fn act(action: &'static str, path: &Path, detail: String) -> Action {
    Action {
        action,
        path: path.display().to_string(),
        detail,
    }
}

fn is_worktree_of(main: &Path, path: &Path) -> bool {
    git::worktrees(main).is_some_and(|all| all.iter().any(|w| same_dir(w, path)))
}

fn same_dir(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}
