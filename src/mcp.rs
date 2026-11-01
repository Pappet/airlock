//! MCP server over stdio (Spec 03): newline-delimited JSON-RPC 2.0.
//!
//! The server is bound to one project and one session for its lifetime, and
//! optionally to one role. Sessions bound to a clean-room role see a reduced
//! tool set restricted to their own role's tasks and their own questions.

use crate::activity::{self, ActivityLog, Channel, Entry};
use crate::app::{
    TaskView, TrackView, claim, done_ids, ensure_role, next_ready, project_root, unix_now,
};
use crate::briefing::{self, Briefing, Via, WorkPackage};
use crate::core::{Priority, Question, QuestionStatus, Task, dependents};
use crate::hub::{Hub, Project};
use crate::store::{NewTask, Store, TaskEdit, TrackEdit};
use crate::sync;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::sync::mpsc;
use std::time::Duration;

/// Supported protocol revisions, newest first. The tool API is the same in all of them.
const PROTOCOL_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Tools exposed to sessions bound to a clean-room role (Spec 03 §2.1).
const CLEAN_ROOM_TOOLS: &[&str] = &[
    "session_start",
    "task_next",
    "inbox",
    "task_get",
    "task_claim",
    "task_heartbeat",
    "task_release",
    "task_complete",
    "question_ask",
    "question_list",
];

const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const PARSE_ERROR: i64 = -32700;
const INTERNAL_ERROR: i64 = -32603;

pub struct Server {
    hub: Hub,
    project: Project,
    session: String,
    role: Option<String>,
    store: Store,
    /// The client's name from `initialize`, recorded with every logged call.
    agent: Option<String>,
}

impl Server {
    pub fn new(hub: Hub, project: Project, session: String, role: Option<String>) -> Result<Self> {
        if let Some(role) = &role {
            ensure_role(&project, role)?;
        }
        let store = Store::open(&project.database)?;
        Ok(Self {
            hub,
            project,
            session,
            role,
            store,
            agent: None,
        })
    }

    /// Serves requests from `input` until it closes, one JSON message per line.
    /// While it runs, the server is listed in the project's process registry.
    pub fn run(&mut self, input: impl BufRead, output: impl Write) -> Result<()> {
        let registration = self.register();
        let result = self.serve(input, output);
        if let Some(registration) = registration {
            registration.end();
        }
        result
    }

    fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> Result<()> {
        for line in input.lines() {
            let line = line.context("reading from stdin")?;
            if line.trim().is_empty() {
                continue;
            }
            let response = match serde_json::from_str::<Value>(&line) {
                Ok(message) => self.handle(message),
                Err(e) => Some(error_response(
                    Value::Null,
                    PARSE_ERROR,
                    &format!("parse error: {e}"),
                )),
            };
            if let Some(response) = response {
                serde_json::to_writer(&mut output, &response)?;
                output.write_all(b"\n")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Lists this server in the process registry and starts its heartbeat.
    /// A failure is reported and the server runs unregistered.
    fn register(&self) -> Option<Registration> {
        let path = ActivityLog::path_for(&self.project.database);
        let registered = ActivityLog::open(&path).and_then(|mut log| {
            let process =
                log.register("mcp", Some(&self.session), self.role.as_deref(), unix_now())?;
            Ok((log, process))
        });
        let (log, process) = match registered {
            Ok(registered) => registered,
            Err(e) => {
                eprintln!("airlock: activity log: {e:#}");
                return None;
            }
        };
        let id = process.id;
        let (stop, stopped) = mpsc::channel::<()>();
        let heartbeat = std::thread::spawn(move || {
            // Its own connection: the one in `log` belongs to the caller.
            let mut log = match ActivityLog::open(&path) {
                Ok(log) => log,
                Err(e) => {
                    eprintln!("airlock: activity log: heartbeat: {e:#}");
                    return;
                }
            };
            while stopped
                .recv_timeout(Duration::from_secs(activity::TOUCH_SECS))
                .is_err_and(|e| e == mpsc::RecvTimeoutError::Timeout)
            {
                let _ = log.touch(&process, unix_now());
            }
        });
        Some(Registration {
            log,
            id,
            stop,
            heartbeat,
        })
    }

    /// Handles one message; returns the response, or `None` for notifications.
    fn handle(&mut self, message: Value) -> Option<Value> {
        let Value::Object(message) = message else {
            return Some(error_response(
                Value::Null,
                INVALID_REQUEST,
                "expected a single JSON-RPC object",
            ));
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // Responses (result or error) answer requests; this server sends none, so they are
            // ignored. Anything else with an id is a malformed request and must be answered.
            let is_response = message.contains_key("result") || message.contains_key("error");
            return id
                .filter(|_| !is_response)
                .map(|id| error_response(id, INVALID_REQUEST, "missing or non-string `method`"));
        };
        // Notifications (e.g. `notifications/initialized`) carry no id and get no response.
        let id = id?;
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

        let result = match method {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": self.tool_definitions() })),
            "tools/call" => self.tools_call(&params),
            "prompts/list" => Ok(json!({ "prompts": [{
                "name": "start",
                "title": "Start Airlock session",
                "description": "Briefing for this session: role, rules, claimed work, next work package.",
            }] })),
            "prompts/get" => self.prompts_get(&params),
            _ => Err((METHOD_NOT_FOUND, format!("method `{method}` not found"))),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err((code, msg)) => error_response(id, code, &msg),
        })
    }

    fn initialize(&mut self, params: &Value) -> Value {
        self.agent = params
            .pointer("/clientInfo/name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let version = requested
            .filter(|v| PROTOCOL_VERSIONS.contains(v))
            .unwrap_or(PROTOCOL_VERSIONS[0]);
        let role = match &self.role {
            Some(role) if self.clean_room() => {
                format!(" You are bound to the clean-room role `{role}`.")
            }
            Some(role) => format!(" You are bound to the role `{role}`."),
            None => String::new(),
        };
        json!({
            "protocolVersion": version,
            "capabilities": { "tools": { "listChanged": false }, "prompts": { "listChanged": false } },
            "serverInfo": { "name": "airlock", "version": crate::VERSION },
            "instructions": format!(
                "Airlock coordinates work on project `{}` as session `{}`.{role} \
                 Call session_start first: it tells you your role, rules, and current work. \
                 Workflow: task_next shows your next work package; task_claim it before starting; \
                 task_heartbeat during long work so the lease does not expire; task_complete when \
                 the Verify step passes. If a spec is ambiguous, ask with question_ask instead of guessing.",
                self.project.config.name, self.session
            ),
        })
    }

    fn prompts_get(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        match params.get("name").and_then(Value::as_str) {
            Some("start") => {}
            Some(name) => return Err((INVALID_PARAMS, format!("unknown prompt `{name}`"))),
            None => return Err((INVALID_PARAMS, "missing prompt name".to_string())),
        }
        let result = self
            .briefing()
            .map_err(|e| (INTERNAL_ERROR, format!("{e:#}")));
        let error = result.as_ref().err().map(|(_, message)| message.as_str());
        self.log_call("prompt start", None, Vec::new(), error);
        let brief = result?;
        Ok(json!({
            "description": "Airlock session briefing",
            "messages": [{
                "role": "user",
                "content": {
                    "type": "text",
                    "text": format!("{}\nBegin with the claimed or next work package.", brief.markdown(Via::Mcp)),
                },
            }],
        }))
    }

    fn briefing(&mut self) -> Result<Briefing> {
        self.project.config = self.hub.load_config(&self.project.id)?;
        // Read before this briefing is logged, so it is the one before this.
        let previous = activity::last_briefing_quietly(&self.project.database, &self.session);
        briefing::build(
            &self.store,
            &self.project,
            &self.session,
            self.role.as_deref(),
            previous,
            unix_now(),
        )
    }

    fn tools_call(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "missing tool name".to_string()))?;
        let empty = Map::new();
        let args = match params.get("arguments") {
            None | Some(Value::Null) => Some(&empty),
            Some(Value::Object(args)) => Some(args),
            Some(_) => None,
        };
        let result = self.run_tool(name, args);
        // Read from the outcome: the returned question says whether this call corrected it.
        let correction = name == "question_answer"
            && result.as_ref().is_ok_and(|result| {
                result["isError"] == false
                    && serde_json::from_value::<Question>(result["structuredContent"].clone())
                        .is_ok_and(|q| activity::corrects(&q))
            });
        let (target, mut changed) = args.map_or((None, Vec::new()), |args| {
            activity::summarize_json(name, args)
        });
        let error = match &result {
            Err((_, message)) => Some(message.as_str()),
            Ok(result) if result["isError"] == true => result["content"][0]["text"].as_str(),
            Ok(_) => None,
        };
        if correction && error.is_none() {
            changed.push("correction".to_string());
        }
        // The inbox is logged by how many items it held, never by what they say.
        let target = match (name, &result) {
            ("inbox", Ok(result)) if error.is_none() => result["structuredContent"]["items"]
                .as_array()
                .map(|items| items.len().to_string()),
            _ => target,
        };
        self.log_call(name, target, changed, error);
        result
    }

    fn run_tool(
        &mut self,
        name: &str,
        args: Option<&Map<String, Value>>,
    ) -> Result<Value, (i64, String)> {
        if !self.tool_available(name) {
            return Err((INVALID_PARAMS, format!("unknown tool `{name}`")));
        }
        let Some(args) = args else {
            return Err((INVALID_PARAMS, "arguments must be an object".to_string()));
        };
        if name == "session_start" {
            // The briefing reads best as Markdown; the structure is in structuredContent.
            return Ok(match self.briefing() {
                Ok(brief) => json!({
                    "content": [{ "type": "text", "text": brief.markdown(Via::Mcp) }],
                    "structuredContent": brief,
                    "isError": false,
                }),
                Err(e) => {
                    json!({ "content": [{ "type": "text", "text": format!("{e:#}") }], "isError": true })
                }
            });
        }
        // Execution and argument errors are tool results, so the model can correct itself.
        Ok(match self.call(name, args) {
            Ok(structured) => json!({
                "content": [{ "type": "text", "text": structured.to_string() }],
                "structuredContent": structured,
                "isError": false,
            }),
            Err(e) => json!({
                "content": [{ "type": "text", "text": format!("{e:#}") }],
                "isError": true,
            }),
        })
    }

    /// Records one call in the activity log; `error` is `None` for a success.
    /// Logging never changes the response: a failure only reaches stderr.
    fn log_call(
        &self,
        command: &str,
        target: Option<String>,
        changed: Vec<String>,
        error: Option<&str>,
    ) {
        let entry = Entry {
            id: 0,
            at: unix_now(),
            session: self.session.clone(),
            role: self.role.clone(),
            clean_room: self.clean_room(),
            channel: Channel::Mcp,
            agent: self.agent.clone(),
            command: command.to_string(),
            target,
            changed,
            write: activity::is_write(command),
            ok: error.is_none(),
            error: error.map(str::to_string),
            pid: std::process::id(),
            build: activity::build().to_string(),
        };
        activity::record_quietly(&self.project.database, &entry);
    }

    fn call(&mut self, name: &str, args: &Map<String, Value>) -> Result<Value> {
        // Roles may change through `airlock import` while the server runs.
        self.project.config = self.hub.load_config(&self.project.id)?;
        let now = unix_now();

        match name {
            "task_next" => {
                let role = self.role_arg(args)?;
                let tasks = self.store.tasks(now)?;
                let tracks = self.store.tracks()?;
                Ok(
                    json!({ "task": next_ready(&tasks, &tracks, &role).map(|v| WorkPackage::from(&v)) }),
                )
            }
            "inbox" => {
                let role = self.role_arg(args)?;
                let peek = opt_bool(args, "peek")?.unwrap_or(false);
                let coordinating = !self
                    .project
                    .config
                    .roles
                    .get(&role)
                    .is_some_and(|r| r.clean_room);
                let inbox = self
                    .store
                    .inbox(&self.session, &role, coordinating, peek, now)?;
                Ok(serde_json::to_value(inbox)?)
            }
            "task_get" => {
                let id = str_arg(args, "task_id")?;
                let tasks = self.store.tasks(now)?;
                let task = tasks
                    .iter()
                    .find(|t| t.id == id)
                    .with_context(|| format!("no task `{id}`"))?;
                if self.clean_room() {
                    self.ensure_own_role(task)?;
                }
                let mut view = TaskView::new(task, &done_ids(&tasks));
                view.dependents = Some(
                    dependents(&tasks, &id)
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                );
                Ok(serde_json::to_value(&view)?)
            }
            "task_claim" | "task_heartbeat" => {
                let id = str_arg(args, "task_id")?;
                self.ensure_own_role(&self.store.task(&id, now)?)?;
                let minutes =
                    opt_u32(args, "lease_minutes")?.unwrap_or(self.project.config.lease_minutes);
                ensure!(minutes > 0, "lease_minutes must be at least 1");
                let lease = i64::from(minutes) * 60;
                if name == "task_heartbeat" {
                    let notes = opt_str(args, "notes")?;
                    let task =
                        self.store
                            .heartbeat(&id, &self.session, lease, notes.as_deref(), now)?;
                    return Ok(json!({
                        "task_id": task.id,
                        "lease_expires_at": task.lease_expires_at,
                        "progress": task.progress,
                    }));
                }
                claim(&mut self.store, &id, &self.session, lease, now)?.to_json()
            }
            "task_release" => {
                let id = str_arg(args, "task_id")?;
                self.ensure_own_role(&self.store.task(&id, now)?)?;
                let task = self.store.release(&id, &self.session, false, now)?;
                Ok(json!({ "task_id": task.id }))
            }
            "task_complete" => {
                let id = str_arg(args, "task_id")?;
                self.ensure_own_role(&self.store.task(&id, now)?)?;
                let notes = opt_str(args, "notes")?;
                let review = opt_bool(args, "review")?.unwrap_or(false);
                let done = self.store.complete(
                    &id,
                    &self.session,
                    notes.as_deref(),
                    review,
                    false,
                    now,
                )?;
                Ok(
                    json!({ "task_id": done.task.id, "state": done.task.state, "now_ready": done.now_ready }),
                )
            }
            "task_create" => {
                ensure!(
                    !args.contains_key("id"),
                    "task ids are assigned by Airlock; omit `id` (the result carries the new id)"
                );
                let role = str_arg(args, "role")?;
                let new = NewTask {
                    prefix: self.project.config.task_prefix(&role)?,
                    title: str_arg(args, "title")?,
                    role,
                    priority: opt_priority(args, "priority")?.unwrap_or_default(),
                    track: opt_str(args, "track")?,
                    blocked_by: str_list(args, "blocked_by")?,
                    read: str_list(args, "read")?,
                    touch: str_list(args, "touch")?,
                    r#do: str_arg(args, "do")?,
                    verify: str_arg(args, "verify")?,
                };
                let task = self.store.add_task(new, now)?;
                let tasks = self.store.tasks(now)?;
                Ok(serde_json::to_value(TaskView::new(
                    &task,
                    &done_ids(&tasks),
                ))?)
            }
            "task_update" => {
                let id = str_arg(args, "task_id")?;
                let edit = TaskEdit {
                    title: opt_str(args, "title")?,
                    role: opt_str(args, "role")?,
                    priority: opt_priority(args, "priority")?,
                    track: opt_nullable_str(args, "track")?,
                    blocked_by: opt_str_list(args, "blocked_by")?,
                    read: opt_str_list(args, "read")?,
                    touch: opt_str_list(args, "touch")?,
                    r#do: opt_str(args, "do")?,
                    verify: opt_str(args, "verify")?,
                };
                if let Some(role) = &edit.role {
                    ensure_role(&self.project, role)?;
                }
                let force = opt_bool(args, "force")?.unwrap_or(false);
                let task = self.store.edit_task(&id, edit, force, now)?;
                let tasks = self.store.tasks(now)?;
                Ok(serde_json::to_value(TaskView::new(
                    &task,
                    &done_ids(&tasks),
                ))?)
            }
            "track_list" => {
                let views = TrackView::list(&self.store.tracks()?, &self.store.tasks(now)?);
                Ok(json!({ "tracks": views }))
            }
            "track_create" => {
                let name = str_arg(args, "name")?;
                let priority = opt_priority(args, "priority")?.unwrap_or_default();
                let description = opt_str(args, "description")?.unwrap_or_default();
                let track = self.store.add_track(&name, priority, &description, now)?;
                Ok(serde_json::to_value(track)?)
            }
            "track_update" => {
                let name = str_arg(args, "name")?;
                let edit = TrackEdit {
                    name: opt_str(args, "new_name")?,
                    priority: opt_priority(args, "priority")?,
                    description: opt_str(args, "description")?,
                };
                Ok(serde_json::to_value(
                    self.store.edit_track(&name, edit, now)?,
                )?)
            }
            "track_remove" => {
                let name = str_arg(args, "name")?;
                self.store.remove_track(&name, now)?;
                Ok(json!({ "removed": name }))
            }
            "question_ask" => {
                let topic = str_arg(args, "topic")?;
                let question = str_arg(args, "question")?;
                let q =
                    self.store
                        .ask(&topic, &question, &self.session, self.role.as_deref(), now)?;
                Ok(json!({ "question_id": q.id }))
            }
            "question_list" => {
                let topic = opt_str(args, "topic")?;
                let status = match opt_str(args, "status")? {
                    Some(s) => Some(
                        serde_json::from_value::<QuestionStatus>(Value::String(s.clone()))
                            .map_err(|_| {
                                anyhow::anyhow!(
                                    "invalid status `{s}`: use open, answered, or resolved"
                                )
                            })?,
                    ),
                    None => None,
                };
                let own_only = self.clean_room();
                let questions: Vec<_> = self
                    .store
                    .questions()?
                    .into_iter()
                    .filter(|q| !own_only || q.asked_by == self.session)
                    .filter(|q| topic.as_deref().is_none_or(|t| q.topic == t))
                    .filter(|q| status.is_none_or(|s| q.status == s))
                    .collect();
                Ok(json!({ "questions": questions }))
            }
            "question_answer" => {
                let id = u64_arg(args, "question_id")?;
                let answer = str_arg(args, "answer")?;
                let spec = opt_str(args, "spec_file")?;
                Ok(serde_json::to_value(self.store.answer(
                    id,
                    &answer,
                    &self.session,
                    spec.as_deref(),
                    now,
                )?)?)
            }
            "question_resolve" => {
                let id = u64_arg(args, "question_id")?;
                let spec = opt_str(args, "spec_file")?;
                Ok(serde_json::to_value(self.store.resolve(
                    id,
                    spec.as_deref(),
                    now,
                )?)?)
            }
            "doc_sync" => {
                let root = project_root(&self.project, "mcp")?;
                let report = sync::sync(&mut self.store, &self.project, root, false, now)?;
                Ok(serde_json::to_value(report)?)
            }
            _ => bail!("unknown tool `{name}`"),
        }
    }

    fn clean_room(&self) -> bool {
        self.role
            .as_ref()
            .and_then(|role| self.project.config.roles.get(role))
            .is_some_and(|role| role.clean_room)
    }

    fn tool_available(&self, name: &str) -> bool {
        tool_definitions(self.role.is_some())
            .iter()
            .any(|t| t["name"] == name && (!self.clean_room() || CLEAN_ROOM_TOOLS.contains(&name)))
    }

    fn tool_definitions(&self) -> Vec<Value> {
        let clean_room = self.clean_room();
        tool_definitions(self.role.is_some())
            .into_iter()
            .filter(|t| !clean_room || CLEAN_ROOM_TOOLS.iter().any(|name| t["name"] == *name))
            .collect()
    }

    /// The role for `task_next`: the bound role, or the required `role` argument.
    fn role_arg(&self, args: &Map<String, Value>) -> Result<String> {
        let requested = opt_str(args, "role")?;
        match (&self.role, requested) {
            (Some(bound), Some(r)) if *bound != r => {
                bail!("this server is bound to role `{bound}`, not `{r}`")
            }
            (Some(bound), _) => Ok(bound.clone()),
            (None, Some(r)) => {
                ensure_role(&self.project, &r)?;
                Ok(r)
            }
            (None, None) => bail!("missing required argument `role`"),
        }
    }

    /// Bound sessions work only on tasks of their own role. Clean-room
    /// sessions are not told which role a foreign task belongs to.
    fn ensure_own_role(&self, task: &Task) -> Result<()> {
        match &self.role {
            Some(role) if task.role != *role && self.clean_room() => {
                bail!("task `{}` is not available to role `{role}`", task.id)
            }
            Some(role) if task.role != *role => bail!(
                "task `{}` belongs to role `{}`; this server is bound to `{role}`",
                task.id,
                task.role
            ),
            _ => Ok(()),
        }
    }
}

/// This server's row in the process registry and the thread keeping it fresh.
struct Registration {
    log: ActivityLog,
    id: u64,
    stop: mpsc::Sender<()>,
    heartbeat: std::thread::JoinHandle<()>,
}

impl Registration {
    /// Stops the heartbeat and removes the registry row.
    fn end(mut self) {
        drop(self.stop);
        let _ = self.heartbeat.join();
        if let Err(e) = self.log.unregister(self.id) {
            eprintln!("airlock: activity log: {e:#}");
        }
    }
}

fn tool_definitions(role_bound: bool) -> Vec<Value> {
    let task_id = json!({ "type": "string", "description": "Task id, e.g. \"E-6d\"" });
    let lease = json!({ "type": "integer", "minimum": 1, "description": "Lease duration in minutes (default: project setting)" });
    let question_id = json!({ "type": "integer", "minimum": 1 });
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    let role_required: &[&str] = if role_bound { &[] } else { &["role"] };

    vec![
        tool(
            "session_start",
            "Call first: your session briefing with role, rules, files to read, claimed work, the next work package, and answers to your questions.",
            json!({}),
            &[],
        ),
        tool(
            "task_next",
            "Show the next ready work package for a role (not claimed yet). Returns {task: null} if none is ready.",
            json!({ "role": { "type": "string", "description": "Role to fetch work for; fixed when the server is role-bound" } }),
            role_required,
        ),
        tool(
            "inbox",
            "What changed for your role since you last looked: new or reprioritized ready tasks, answers to your questions, rejected reviews; for coordinating roles also new questions and tasks in review. Marks them as read unless peek is set.",
            json!({
                "role": { "type": "string", "description": "Role to read for; fixed when the server is role-bound" },
                "peek": { "type": "boolean", "description": "Show without marking as read" }
            }),
            role_required,
        ),
        tool(
            "task_get",
            "Show one task in full: instructions, effective status, prerequisites, dependents, lease.",
            json!({ "task_id": task_id }),
            &["task_id"],
        ),
        tool(
            "task_claim",
            "Claim a ready task for this session and start a lease. Claim before starting work.",
            json!({ "task_id": task_id, "lease_minutes": lease }),
            &["task_id"],
        ),
        tool(
            "task_heartbeat",
            "Extend this session's lease on a claimed task, counted from now. Before your context fills, \
             record where you are in `notes`; your successor reads it in its briefing.",
            json!({
                "task_id": task_id,
                "lease_minutes": lease,
                "notes": {
                    "type": "string",
                    "description": "Running note for your successor: done so far, remaining, next step. \
                                    Replaces the previous note; an empty string removes it."
                }
            }),
            &["task_id"],
        ),
        tool(
            "task_release",
            "Give a task claimed by this session back, unfinished.",
            json!({ "task_id": task_id }),
            &["task_id"],
        ),
        tool(
            "task_complete",
            "Finish a task claimed by this session once its Verify step passes. Returns the tasks that became ready.",
            json!({
                "task_id": task_id,
                "notes": { "type": "string", "description": "What was done and how it was verified" },
                "review": { "type": "boolean", "description": "Submit for human review instead of marking done" },
            }),
            &["task_id"],
        ),
        tool(
            "task_create",
            "Create a work package; Airlock assigns its id (role prefix + next number) and returns it. Prerequisites in blocked_by must already exist.",
            json!({
                "title": { "type": "string" },
                "role": { "type": "string" },
                "do": { "type": "string", "description": "Concrete behavioral instructions" },
                "verify": { "type": "string", "description": "Test command or scenario proving completion" },
                "read": strings,
                "touch": strings,
                "blocked_by": strings,
                "priority": { "type": "string", "enum": ["critical", "high", "normal", "low"] },
                "track": { "type": "string", "description": "Track the task belongs to; create it first with track_create" },
            }),
            &["title", "role", "do", "verify"],
        ),
        tool(
            "task_update",
            "Change the content of a work package. Omitted fields are kept; a list replaces the old one ([] empties it). Tasks past open need force.",
            json!({
                "task_id": task_id,
                "title": { "type": "string" },
                "role": { "type": "string" },
                "do": { "type": "string", "description": "Concrete behavioral instructions" },
                "verify": { "type": "string", "description": "Test command or scenario proving completion" },
                "read": strings,
                "touch": strings,
                "blocked_by": strings,
                "priority": { "type": "string", "enum": ["critical", "high", "normal", "low"] },
                "track": { "type": ["string", "null"], "description": "Track to move the task to; null takes it out of its track" },
                "force": { "type": "boolean", "description": "Edit a task that is in progress, in review, or done" },
            }),
            &["task_id"],
        ),
        tool(
            "track_list",
            "List tracks, most important first, with the number of their tasks per status (blocked, ready, in_progress, in_review, done).",
            json!({}),
            &[],
        ),
        tool(
            "track_create",
            "Create a track: a named line of work with its own priority. Ready tasks of more important tracks are handed out first.",
            json!({
                "name": { "type": "string", "description": "Letters, digits, `-`, `_`" },
                "priority": { "type": "string", "enum": ["critical", "high", "normal", "low"] },
                "description": { "type": "string" },
            }),
            &["name"],
        ),
        tool(
            "track_update",
            "Rename or change a track. Omitted fields are kept; the tasks of a renamed track follow it.",
            json!({
                "name": { "type": "string", "description": "Current name" },
                "new_name": { "type": "string" },
                "priority": { "type": "string", "enum": ["critical", "high", "normal", "low"] },
                "description": { "type": "string" },
            }),
            &["name"],
        ),
        tool(
            "track_remove",
            "Remove a track no task belongs to.",
            json!({ "name": { "type": "string" } }),
            &["name"],
        ),
        tool(
            "question_ask",
            "Ask a behavioral question about a spec instead of guessing. Never include implementation code.",
            json!({
                "topic": { "type": "string", "description": "Spec file or subsystem, e.g. \"melee.md\"" },
                "question": { "type": "string" },
            }),
            &["topic", "question"],
        ),
        tool(
            "question_list",
            "List questions and their answers, newest first.",
            json!({
                "topic": { "type": "string" },
                "status": { "type": "string", "enum": ["open", "answered", "resolved"] },
            }),
            &[],
        ),
        tool(
            "question_answer",
            "Answer a question in behavioral prose. Answering one that already has an answer, resolved or not, corrects it; the earlier answer is kept.",
            json!({
                "question_id": question_id,
                "answer": { "type": "string" },
                "spec_file": { "type": "string", "description": "Spec file in which the answer was codified" },
            }),
            &["question_id", "answer"],
        ),
        tool(
            "question_resolve",
            "Close an answered question.",
            json!({ "question_id": question_id, "spec_file": { "type": "string" } }),
            &["question_id"],
        ),
        tool(
            "doc_sync",
            "Export the state snapshot and Markdown views into the project root (like `airlock sync`).",
            json!({}),
            &[],
        ),
    ]
}

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn str_arg(args: &Map<String, Value>, key: &str) -> Result<String> {
    opt_str(args, key)?.with_context(|| format!("missing required argument `{key}`"))
}

fn opt_str(args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => bail!("argument `{key}` must be a string"),
    }
}

/// A string that may be `null` on purpose: absent keeps (`None`), `null` clears
/// (`Some(None)`), a string sets (`Some(Some(..))`).
fn opt_nullable_str(args: &Map<String, Value>, key: &str) -> Result<Option<Option<String>>> {
    match args.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(Value::String(s)) => Ok(Some(Some(s.clone()))),
        Some(_) => bail!("argument `{key}` must be a string or null"),
    }
}

fn opt_bool(args: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => bail!("argument `{key}` must be a boolean"),
    }
}

fn opt_i64(args: &Map<String, Value>, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .map(Some)
            .with_context(|| format!("argument `{key}` must be an integer")),
    }
}

/// A priority level by name; anything else, numbers included, is rejected
/// with the message listing the four levels.
fn opt_priority(args: &Map<String, Value>, key: &str) -> Result<Option<Priority>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.parse()?)),
        Some(other) => Ok(Some(other.to_string().parse()?)),
    }
}

fn opt_u32(args: &Map<String, Value>, key: &str) -> Result<Option<u32>> {
    opt_i64(args, key)?
        .map(|v| {
            u32::try_from(v).with_context(|| format!("argument `{key}` must be a positive integer"))
        })
        .transpose()
}

fn u64_arg(args: &Map<String, Value>, key: &str) -> Result<u64> {
    let v = opt_i64(args, key)?.with_context(|| format!("missing required argument `{key}`"))?;
    u64::try_from(v).with_context(|| format!("argument `{key}` must be a positive integer"))
}

fn str_list(args: &Map<String, Value>, key: &str) -> Result<Vec<String>> {
    Ok(opt_str_list(args, key)?.unwrap_or_default())
}

fn opt_str_list(args: &Map<String, Value>, key: &str) -> Result<Option<Vec<String>>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| {
                i.as_str()
                    .map(str::to_string)
                    .with_context(|| format!("`{key}` must contain only strings"))
            })
            .collect::<Result<_>>()
            .map(Some),
        Some(_) => bail!("argument `{key}` must be an array of strings"),
    }
}
