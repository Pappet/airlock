//! `airlock setup <harness>`: prints MCP configurations and Claude Code hooks for agent
//! harnesses (Spec 06 §6).

use crate::app::{Output, ensure_role, project_root};
use crate::cli::Harness;
use crate::hub::{Project, binding_key};
use anyhow::{Context, Result, ensure};
use serde_json::json;

pub(crate) fn run(
    project: &Project,
    harness: Harness,
    role: Option<String>,
    auto_continue: bool,
    out: &Output,
) -> Result<()> {
    ensure!(
        !auto_continue || harness == Harness::ClaudeHooks,
        "--auto-continue applies to `airlock setup claude-hooks` only"
    );
    let root = project_root(project, "setup")?;
    let root = binding_key(root)?;
    let binding = project.binding.as_ref();
    let role = role
        .or_else(|| binding.map(|b| b.role.clone()))
        .context("no role: pass --role or bind this worktree with `airlock use <role>`")?;
    ensure_role(project, &role)?;
    let from_binding = binding.is_some_and(|b| b.role == role);
    // The Stop hook acts for the worktree's binding, so it needs one for this role.
    ensure!(
        !auto_continue || from_binding,
        "--auto-continue needs this worktree bound to role `{role}` (`airlock use {role}`): \
         the Stop hook reads the inbox of the binding's session"
    );
    let session = match binding {
        Some(b) if from_binding => b.session.clone(),
        _ => {
            let dir = std::path::Path::new(&root)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("worktree");
            format!("{role}@{dir}")
        }
    };
    let bin = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("locating the airlock binary")?
        .to_string_lossy()
        .into_owned();

    // Per-directory harnesses read role and session from the binding when it matches;
    // global configurations carry the worktree, role, and session explicitly.
    let local_args: Vec<String> = if from_binding {
        vec!["mcp".into()]
    } else {
        vec![
            "mcp".into(),
            "--role".into(),
            role.clone(),
            "--session".into(),
            session.clone(),
        ]
    };
    let global_args: Vec<String> = vec![
        "-C".into(),
        root.clone(),
        "mcp".into(),
        "--role".into(),
        role.clone(),
        "--session".into(),
        session.clone(),
    ];

    // The hook briefs the same identity the MCP server acts as: role and session come from the
    // binding when it matches, otherwise they are passed like `local_args` above.
    // Claude Code works through the MCP server, so the briefing names MCP tools.
    let mut hook_args: Vec<String> = vec![
        "-C".into(),
        root.clone(),
        "start".into(),
        "--hints".into(),
        "mcp".into(),
    ];
    if !from_binding {
        hook_args.extend([
            "--role".into(),
            role.clone(),
            "--session".into(),
            session.clone(),
        ]);
    }

    // stdout carries only the file content, so redirecting it is always safe; hints go to stderr.
    let (args, text, hint) = match harness {
        Harness::Claude => {
            let mcp_json =
                json!({ "mcpServers": { "airlock": { "command": bin, "args": local_args } } });
            let hint = format!(
                "Save as {root}/.mcp.json (one per worktree). Avoid `claude mcp add --scope local`: \
                 Claude Code applies local-scope servers to every worktree of the repository."
            );
            (local_args, serde_json::to_string_pretty(&mcp_json)?, hint)
        }
        Harness::ClaudeHooks => {
            let command = hook_command(&bin, &hook_args);
            let mut settings = json!({
                "hooks": {
                    "SessionStart": [{
                        "matcher": "startup|resume|clear|compact",
                        "hooks": [{ "type": "command", "command": command, "timeout": 15 }]
                    }]
                }
            });
            let mut hint = format!(
                "Save as {root}/.claude/settings.local.json if that file does not exist; otherwise merge \
                 the `hooks` key into it. The file is local to this worktree and not committed. Claude Code \
                 then runs `airlock start` when a session starts, resumes, is cleared, or is compacted, \
                 and adds the briefing to the model's context."
            );
            if auto_continue {
                let stop = hook_command(
                    &bin,
                    &["-C".into(), root.clone(), "hook".into(), "stop".into()],
                );
                settings["hooks"]["Stop"] =
                    json!([{ "hooks": [{ "type": "command", "command": stop, "timeout": 15 }] }]);
                hint.push_str(
                    " With --auto-continue, Claude Code also runs `airlock hook stop` when the session \
                     ends its turn: if Airlock has news for the role it was not yet told about, the \
                     session continues instead of stopping (at most `auto_continue_limit` times an hour).",
                );
            }
            (hook_args, serde_json::to_string_pretty(&settings)?, hint)
        }
        Harness::Antigravity => {
            let entry = json!({ "mcpServers": { format!("airlock-{role}"): { "command": bin, "args": global_args } } });
            let hint = "Merge the entry into ~/.gemini/antigravity/mcp_config.json.".to_string();
            (global_args, serde_json::to_string_pretty(&entry)?, hint)
        }
        Harness::Dsh => {
            let file = format!("~/.dsh/airlock/{}-{role}.yml", project.config.name);
            let quoted: Vec<String> = global_args.iter().map(|a| yaml_quote(a)).collect();
            let text = format!(
                "# Airlock MCP server for project {name}, role {role} (generated by `airlock setup dsh`).\n\
                 # Start a DSH instance with it — one role per instance:\n\
                 #   dsh web --patch {file} --port 0\n\
                 - insert:\n\
                 \x20   - id: mcp-airlock-{role}\n\
                 \x20     name: '@deepseek-ai/dsh-mcp-client'\n\
                 \x20     config:\n\
                 \x20       serverName: airlock\n\
                 \x20       transport: stdio\n\
                 \x20       command: {cmd}\n\
                 \x20       args: [ {args} ]\n\
                 \x20       toolCallTimeoutMs: 300000",
                name = project.config.name,
                cmd = yaml_quote(&bin),
                args = quoted.join(", "),
            );
            let hint = format!("Save as {file}, then start: dsh web --patch {file} --port 0");
            (global_args, text, hint)
        }
    };

    if !out.json {
        eprintln!("{hint}");
    }
    out.print(
        &json!({ "role": role, "session": session, "root": root, "command": bin, "args": args, "hint": hint }),
        || text,
    )
}

/// What Claude Code expands to the directory a session was started in. Claude Code also runs
/// the hooks in the main checkout's `.claude/settings.local.json` in sessions of its linked
/// worktrees, so a hook names its worktree through this variable, never by path: copied
/// into another worktree's session it then serves that worktree (and Claude Code runs
/// identical commands once).
pub(crate) const PROJECT_DIR_VAR: &str = "\"$CLAUDE_PROJECT_DIR\"";

/// A Claude Code hook command line: `bin` and `args` shell-quoted, except that the value of
/// `-C` becomes [`PROJECT_DIR_VAR`].
fn hook_command(bin: &str, args: &[String]) -> String {
    let mut words = vec![shell_quote(bin)];
    let mut after_c = false;
    for arg in args {
        words.push(if after_c {
            PROJECT_DIR_VAR.to_string()
        } else {
            shell_quote(arg)
        });
        after_c = arg == "-C";
    }
    words.join(" ")
}

/// Whether a `-C` value read back from a hook command names the session's own directory.
pub(crate) fn is_project_dir_var(word: &str) -> bool {
    matches!(word, "$CLAUDE_PROJECT_DIR" | "${CLAUDE_PROJECT_DIR}")
}

/// A word for a POSIX shell command line: unchanged when it holds only safe characters,
/// otherwise single-quoted.
pub(crate) fn shell_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_-./:@=+,%".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Splits a POSIX shell command line into words (single and double quotes, backslash escapes);
/// enough to read back what `shell_quote` writes and what people type by hand.
pub(crate) fn shell_words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                word.extend(chars.by_ref().take_while(|&c| c != '\''));
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => word.extend(chars.next()),
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                word.extend(chars.next());
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}

/// A YAML single-quoted scalar.
fn yaml_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::{shell_quote, shell_words};

    #[test]
    fn quoted_words_read_back() {
        let words = [
            "/usr/bin/airlock",
            "/home/me/My Projects/it's here",
            "",
            "C:\\Users\\me\\airlock.exe",
            "lead@openshr",
        ];
        let line = words
            .iter()
            .map(|w| shell_quote(w))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(shell_words(&line), words);
        assert_eq!(shell_quote("lead@openshr"), "lead@openshr");
    }

    #[test]
    fn hand_typed_lines_split_like_a_shell() {
        assert_eq!(
            shell_words(r#"airlock -C "/a b" start  --role\ x 'y z'"#),
            ["airlock", "-C", "/a b", "start", "--role x", "y z"]
        );
    }
}
