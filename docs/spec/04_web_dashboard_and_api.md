# Spec 04: Web Dashboard & REST/SSE API

## 1. Overview

`airlock serve` launches an embedded web application and REST API on `localhost:3000` (configurable).

The web dashboard is delivered as a zero-dependency single-binary web service: all HTML, CSS, and client-side JavaScript assets are embedded directly into the compiled Rust binary.

---

## 2. Web Dashboard Features

### 2.1 Visual Kanban Board
- **Columns**: `Backlog`, `Ready`, `In Progress`, `In Review`, `Done`.
- **Role Filters**: Filter cards by `Lead`, `Spec`, `Engine`, or custom roles defined in `.airlock.json`.
- **Task Cards**: Displays Task ID, title, claimed session, and dependency badges. Clicking a card opens a modal with full *Read*, *Touch*, *Do*, and *Verify* instructions.

### 2.2 Airlock / Questions Inbox
- Displays open questions submitted by clean-room or implementation sessions.
- Allows the human project lead to directly read, answer, or assign questions.
- Tracks question status: `Open`, `Answered`, `Resolved`.

### 2.3 Human-in-the-Loop Task Queue
- Dedicated section for tasks flagged for manual human verification (e.g. "Test in Bottles", "Verify graphical rendering on hardware", "Code review PR").
- Provides one-click verification and resolution.

### 2.4 Live Concurrency Monitor
- Real-time display of active sessions holding task leases.
- Shows lease duration and time remaining before automatic lease expiry.

---

## 3. REST & Event API

All endpoints communicate via JSON over HTTP.

### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/tasks` | List all tasks (supports query params `?role=...&status=...`) |
| `POST` | `/api/tasks` | Create a new task |
| `GET` | `/api/tasks/:id` | Get single task details |
| `POST` | `/api/tasks/:id/claim` | Claim task lease |
| `POST` | `/api/tasks/:id/done` | Mark task done |
| `GET` | `/api/questions` | List questions |
| `POST` | `/api/questions` | Submit new question |
| `POST` | `/api/questions/:id/answer`| Answer question |
| `POST` | `/api/sync` | Trigger documentation sync |
| `GET` | `/api/events` | Server-Sent Events (SSE) stream for real-time UI updates |
