# Phase Plan: Airlock Development

This document outlines the phased implementation plan for Airlock.

---

## Phase 1: Core Domain Model & CLI Foundation (Current)
- [x] Initial project setup (Rust edition 2024, Cargo manifest with locked dependencies).
- [x] Core data models (`Task`, `Question`, `ProjectConfig`, `TaskStatus`, `QuestionStatus`).
- [x] Behavioral specifications (`docs/spec/01` - `05`).
- [ ] SQLite persistence layer (`rusqlite` with WAL mode).
- [ ] Functional CLI commands: `airlock init`, `airlock task list/next/claim/done`, `airlock question ask/list/answer`.
- [ ] Integration and smoke test suite in `tests/`.

---

## Phase 2: Markdown Sync & Importer
- [ ] Parser for existing monolithic `PLAN.md` files (importing existing OpenSHR `L-...` and `E-...` packages directly).
- [ ] Markdown generator for `PLAN.md` and `QUESTIONS.md`.
- [ ] Automated dependency graph resolution (marking tasks `Ready` when blockers reach `Done`).

---

## Phase 3: MCP Server (Model Context Protocol)
- [ ] Stdio-based JSON-RPC / MCP server protocol implementation.
- [ ] Agent tools: `task_next`, `task_claim`, `task_complete`, `question_ask`, `question_answer`.
- [ ] Agent testing with Claude Code and Antigravity.

---

## Phase 4: Embedded Web Dashboard & Server
- [ ] HTTP & SSE server using `axum`.
- [ ] Single-page interactive Kanban dashboard (vanilla JS/Tailwind or Preact).
- [ ] Asset embedding using `rust-embed`.
- [ ] Real-time updates via Server-Sent Events.

---

## Phase 5: Production Polish & Multi-Project Packaging
- [ ] Global CLI installation (`cargo install --path .`).
- [ ] Pre-packaged templates for different workflows (Clean-Room RE, Web App Fullstack, Research & Spec).
- [ ] Comprehensive documentation and walkthrough guides.
