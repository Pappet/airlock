# Spec 03: MCP Protocol (Model Context Protocol)

## 1. Overview

The Model Context Protocol (MCP) server allows AI assistants (Claude, Antigravity, Cursor, etc.) to interact directly with Airlock using typed tool calls rather than parsing command-line text.

When configured in an assistant's settings (e.g. `.claude/settings.json` or Antigravity MCP config), the assistant gains a specialized set of tools for workflow coordination.

---

## 2. MCP Tools Specification

### 2.1 `task_next`
Returns the next actionable task for the agent's assigned role.

- **Parameters**:
  - `role` (string, required): Role of the requesting session (e.g. `"engine"`, `"lead"`).
- **Returns**:
  Structured object containing `id`, `title`, `read`, `touch`, `do`, `verify`, and `blocked_by`.
- **Token Efficiency**:
  Outputs under 200 tokens of focused instructions, bypassing the need to ingest large project plan documents.

### 2.2 `task_claim`
Claims a work package and starts a session lease.

- **Parameters**:
  - `task_id` (string, required): The ID of the task to claim (e.g. `"E-6d"`).
  - `session_id` (string, optional): Unique name/ID for the agent session.
  - `lease_minutes` (integer, optional, default: 60): Lease duration.
- **Returns**:
  Confirmation of claim or an error if the task is already claimed by another active session.

### 2.3 `task_complete`
Completes a task, recording verification details and unblocking dependent tasks.

- **Parameters**:
  - `task_id` (string, required): The ID of the task.
  - `notes` (string, optional): Summary of changes, tests executed, or status notes.
- **Returns**:
  List of newly unblocked task IDs.

### 2.4 `task_create`
Allows lead or architectural sessions to add or break down work packages.

- **Parameters**:
  - `id` (string, required)
  - `title` (string, required)
  - `role` (string, required)
  - `read` (array of strings, optional)
  - `touch` (array of strings, optional)
  - `do` (string, required)
  - `verify` (string, required)
  - `blocked_by` (array of strings, optional)
- **Returns**:
  Created task summary.

### 2.5 `question_ask`
Submits a question to the airlock when an engine session encounters an ambiguous or incomplete specification.

- **Parameters**:
  - `topic` (string, required): Name of the spec or component (e.g. `"melee.md"`).
  - `question` (string, required): Concrete description of the question.
- **Returns**:
  Question ID and acknowledgement.

### 2.6 `question_answer`
Allows lead sessions to provide authoritative answers to pending questions.

- **Parameters**:
  - `question_id` (integer, required): ID of the question.
  - `answer` (string, required): Prose explanation or specification resolution.
  - `spec_file` (string, optional): The specification file where the resolution was codified.
- **Returns**:
  Updated question state.

### 2.7 `doc_sync`
Triggers generation of project markdown documents from the live database.

- **Returns**:
  Status and list of updated files.
