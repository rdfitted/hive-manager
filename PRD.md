# Hive Manager - Product Requirements Document

## Overview

**Product Name:** Hive Manager
**Version:** 1.0.0
**Platform:** Windows (primary), macOS/Linux (future)
**Tech Stack:** Tauri v2 + Rust backend + Svelte frontend

### Vision

A native desktop application for orchestrating and monitoring Claude Code multi-agent workflows (Hives, Swarms, and Fusions). Replaces Windows Terminal tab sprawl with a unified interface featuring embedded terminals, real-time hierarchy visualization, and cross-agent coordination.

### Problem Statement

Currently, multi-agent workflows spawn 10-20+ Windows Terminal tabs:
- Queen, Planners (A-D), Workers (1a-4a, 1b-4b...), Reviewers, Testers, Integration Team
- No unified view of session hierarchy or state
- Easy to miss agent prompts buried in background tabs
- Manual tracking of which agent owns which domain/files
- Coordination logs scattered across files
- Different CLIs (claude, agent, gemini, opencode, codex) with different behaviors

### Solution

A single-window application that:
- Embeds all agent terminals in a managed grid/tree layout
- Visualizes the Queen → Planner → Worker hierarchy in real-time
- Alerts when any agent requests input (across all CLI types)
- Parses coordination.log and displays structured state
- Tracks file ownership matrix visually
- Supports steering individual agents mid-task

---

## Agent Architecture Support

### Tier 1: Queen (1 per session)
| Property | Value |
|----------|-------|
| CLI | `claude --model opus` |
| Role | Top-level orchestrator |
| Spawns | Planners only |
| Owns | Branch, commits, PR, integration |
| Can Modify Code | No |

### Tier 2: Planners (1-10 per session)
| Property | Value |
|----------|-------|
| CLI | `claude --model opus` |
| Role | Domain orchestrator |
| Spawns | Workers, Reviewer, Tester |
| Owns | Task files, worker coordination, review cycle |
| Can Modify Code | No |

### Tier 3: Workers (2-4 per Planner)
| Worker | CLI | Model | Role |
|--------|-----|-------|------|
| Worker 1X (Backend) | `agent` via WSL | Cursor/Opus | Backend implementation |
| Worker 2X (Frontend) | `gemini` | Gemini 3 Pro | Frontend implementation |
| Worker 3X (Coherence) | `opencode` | Grok Code | Cross-cutting coherence |
| Worker 4X (Simplify) | `codex` | GPT-5.2 | Simplification pass |

### Support Agents
| Agent | CLI | Model | Role |
|-------|-----|-------|------|
| Reviewer | `opencode` | BigPickle | Code review |
| Tester | `codex` | GPT-5.2 | Test execution |
| Integration Reviewer | `opencode` | BigPickle | Cross-domain review |
| Integration Tester | `codex` | GPT-5.2 | Integration tests |

### CLI Reference
| CLI | Auto-Approve Flag | Model Flag | Platform |
|-----|-------------------|------------|----------|
| `claude` | `--dangerously-skip-permissions` | `--model opus` | Windows |
| `agent` | `--force` | (global) | WSL Ubuntu |
| `gemini` | `-y` | `-m gemini-3-pro-preview` | Windows |
| `opencode` | env `OPENCODE_YOLO=true` | `-m opencode/MODEL` | Windows |
| `codex` | `--dangerously-bypass-approvals-and-sandbox` | `-m gpt-5.3-codex` | Windows |

---

## Session Types

### Hive (B-Thread)
```
Queen (Opus)
├── Worker 1 (mixed)
├── Worker 2 (mixed)
├── Worker 3 (mixed)
└── Worker 4 (mixed)
    └── Reviewer + Tester
```
- 1 Queen + 1-4 Workers
- Single domain focus
- Sequential or parallel workers

### Swarm (S-Thread)
```
Queen (Opus)
├── Planner A (Domain A)
│   ├── Worker 1a (Backend)
│   ├── Worker 2a (Frontend)
│   ├── Worker 3a (Coherence)
│   └── Worker 4a (Simplify)
│       └── Reviewer A + Tester A
├── Planner B (Domain B)
│   ├── Worker 1b...
│   └── ...
└── Integration Team
    ├── Integration Reviewer
    └── Integration Tester
```
- 1 Queen + 2-4 Planners + 2-4 Workers each
- Multi-domain parallel execution
- File ownership matrix prevents conflicts

### Swarm Long-Horizon
- Same as Swarm but Planners deployed in waves (1-2 at a time)
- Later Planners benefit from earlier discoveries
- Up to 10 Planners (A-J)

### Fusion (F-Thread)
```
Judge (You)
├── Variant A (worktree)
├── Variant B (worktree)
└── Variant C (worktree)
```
- Competing implementations in separate git worktrees
- Human or automated judge picks winner

---

## Functional Requirements

### F1: Session Management

| ID | Requirement | Priority |
|----|-------------|----------|
| F1.1 | Launch Hive session (Queen + 1-4 Workers) | P0 |
| F1.2 | Launch Swarm session (Queen + 2-4 Planners) | P0 |
| F1.3 | Launch Swarm Long-Horizon (sequential waves) | P1 |
| F1.4 | Launch Fusion session (competing worktrees) | P1 |
| F1.5 | Stop individual agent | P0 |
| F1.6 | Stop entire session (cascade) | P0 |
| F1.7 | View session history | P1 |
| F1.8 | Resume interrupted session | P2 |
| F1.9 | Clone session config for new task | P2 |

### F2: Embedded Terminals (Multi-CLI Support)

| ID | Requirement | Priority |
|----|-------------|----------|
| F2.1 | Spawn and render `claude` CLI via PTY | P0 |
| F2.2 | Spawn and render `agent` via WSL PTY | P0 |
| F2.3 | Spawn and render `gemini` CLI via PTY | P0 |
| F2.4 | Spawn and render `opencode` CLI via PTY | P0 |
| F2.5 | Spawn and render `codex` CLI via PTY | P0 |
| F2.6 | Full ANSI color/formatting support | P0 |
| F2.7 | Send keyboard input to focused terminal | P0 |
| F2.8 | Terminal resize (responsive panels) | P0 |
| F2.9 | Search within terminal output | P1 |
| F2.10 | Copy/paste support | P0 |
| F2.11 | Auto-scroll with lock/unlock | P0 |

### F3: Hierarchy Visualization

| ID | Requirement | Priority |
|----|-------------|----------|
| F3.1 | Display live hierarchy tree (Queen → Planners → Workers) | P0 |
| F3.2 | Show agent status badges (starting, running, waiting, complete, error) | P0 |
| F3.3 | Indicate which agent is focused | P0 |
| F3.4 | Click hierarchy node to focus terminal | P0 |
| F3.5 | Show Planner domain assignment | P1 |
| F3.6 | Show Worker role (Backend/Frontend/Coherence/Simplify) | P1 |
| F3.7 | Collapse/expand hierarchy branches | P1 |

### F4: Session State Monitoring

| ID | Requirement | Priority |
|----|-------------|----------|
| F4.1 | Parse and display `coordination.log` messages | P0 |
| F4.2 | Parse `state/responsibility-matrix.md` | P1 |
| F4.3 | Parse `state/file-ownership.md` and display matrix | P1 |
| F4.4 | Track task files in `tasks/planner-{X}/` | P1 |
| F4.5 | Show wave progress (long-horizon) | P1 |
| F4.6 | Display phase progress (Planning → Execution → Review → Integration) | P1 |

### F5: Agent Interaction

| ID | Requirement | Priority |
|----|-------------|----------|
| F5.1 | Visual + audio alert when any agent requests input | P0 |
| F5.2 | Auto-focus terminal requesting input | P0 |
| F5.3 | Send text input to agent | P0 |
| F5.4 | Quick response buttons (y/n, approve, reject) | P1 |
| F5.5 | Broadcast message to all agents (via coordination.log) | P2 |

### F6: Layout Options

| ID | Requirement | Priority |
|----|-------------|----------|
| F6.1 | Grid layout (2x2, 3x3, 4x4) | P0 |
| F6.2 | Tree layout (hierarchy-based, indented) | P1 |
| F6.3 | Tabbed layout (one terminal at a time) | P0 |
| F6.4 | Focus mode (single terminal fullscreen) | P0 |
| F6.5 | Custom split layouts | P2 |
| F6.6 | Save/restore layout preferences | P1 |

### F7: Configuration

| ID | Requirement | Priority |
|----|-------------|----------|
| F7.1 | Configure default Planner count | P1 |
| F7.2 | Configure Worker models per role | P1 |
| F7.3 | Configure CLI paths and flags | P1 |
| F7.4 | Configure session directory paths | P1 |
| F7.5 | Keyboard shortcut customization | P1 |
| F7.6 | Theme selection (dark/light) | P2 |

### F8: Updates & Maintenance

| ID | Requirement | Priority |
|----|-------------|----------|
| F8.1 | Check for updates on startup | P0 |
| F8.2 | Show changelog before update | P1 |
| F8.3 | Download in background, install on exit | P1 |
| F8.4 | Manual update check via menu | P0 |
| F8.5 | Rollback to previous version | P2 |

---

## Technical Architecture

### System Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                          HIVE MANAGER                               │
├─────────────────────────────────────────────────────────────────────┤
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                      SVELTE FRONTEND                          │  │
│  │  ┌──────────┐  ┌────────────────────┐  ┌──────────────────┐  │  │
│  │  │ Sidebar  │  │   Terminal Grid    │  │   Status Panel   │  │  │
│  │  │          │  │   ┌────┐ ┌────┐    │  │                  │  │  │
│  │  │ Sessions │  │   │ Q  │ │ Pa │    │  │ Hierarchy Tree   │  │  │
│  │  │ History  │  │   └────┘ └────┘    │  │ ├─ Queen         │  │  │
│  │  │ Settings │  │   ┌────┐ ┌────┐    │  │ ├─ Planner A     │  │  │
│  │  │          │  │   │W1a │ │W2a │    │  │ │  ├─ W1a █      │  │  │
│  │  │          │  │   └────┘ └────┘    │  │ │  └─ W2a ⏳     │  │  │
│  │  │ [+] New  │  │      (xterm.js)    │  │ └─ Planner B     │  │  │
│  │  └──────────┘  └────────────────────┘  └──────────────────┘  │  │
│  └───────────────────────────────────────────────────────────────┘  │
│                                │                                     │
│                      Tauri IPC + Events                             │
│                                │                                     │
│  ┌───────────────────────────────────────────────────────────────┐  │
│  │                       RUST BACKEND                            │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌─────────────┐ │  │
│  │  │   PTY Manager    │  │ Session Manager  │  │ File Watch  │ │  │
│  │  │  - Windows PTY   │  │  - Hive          │  │  - notify   │ │  │
│  │  │  - WSL PTY       │  │  - Swarm         │  │  - parser   │ │  │
│  │  │  - Multi-CLI     │  │  - Fusion        │  │             │ │  │
│  │  └──────────────────┘  └──────────────────┘  └─────────────┘ │  │
│  │  ┌──────────────────┐  ┌──────────────────┐  ┌─────────────┐ │  │
│  │  │  Agent Registry  │  │  Config Store    │  │  Updater    │ │  │
│  │  │  - Hierarchy     │  │  - CLI paths     │  │  - GitHub   │ │  │
│  │  │  - Status        │  │  - Defaults      │  │  - Rollback │ │  │
│  │  └──────────────────┘  └──────────────────┘  └─────────────┘ │  │
│  └───────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────┘
                                │
        ┌───────────────────────┼───────────────────────┐
        ▼                       ▼                       ▼
   ┌─────────┐            ┌─────────┐            ┌─────────┐
   │ claude  │            │  agent  │            │ gemini  │
   │  (PTY)  │            │ (WSL)   │            │  (PTY)  │
   └─────────┘            └─────────┘            └─────────┘
        ▼                       ▼                       ▼
   ┌─────────┐            ┌─────────┐
   │opencode │            │  codex  │
   │  (PTY)  │            │  (PTY)  │
   └─────────┘            └─────────┘
```

### Rust Backend Components

#### PTY Manager (`src-tauri/src/pty/`)

```rust
pub struct PtyManager {
    sessions: HashMap<String, PtySession>,
    wsl_available: bool,
}

pub struct PtySession {
    id: String,
    agent_type: AgentType,
    pty: Box<dyn portable_pty::MasterPty>,
    child: Box<dyn portable_pty::Child>,
    status: AgentStatus,
}

pub enum AgentType {
    Claude { model: String },           // claude --model opus
    CursorAgent,                        // WSL: agent --force
    Gemini { model: String },           // gemini -m MODEL -y
    OpenCode { model: String },         // OPENCODE_YOLO=true opencode -m MODEL
    Codex { model: String },            // codex --dangerously-bypass...
}

pub enum AgentStatus {
    Starting,
    Running,
    WaitingForInput,  // Detected via output patterns
    Completed,
    Error(String),
}
```

#### Session Manager (`src-tauri/src/session/`)

```rust
pub struct SessionManager {
    active_sessions: HashMap<String, Session>,
    history: Vec<SessionSummary>,
}

pub struct Session {
    id: String,
    session_type: SessionType,
    project_path: PathBuf,
    agents: AgentHierarchy,
    state: SessionState,
    created_at: DateTime<Utc>,
}

pub enum SessionType {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    SwarmLongHorizon { max_planners: u8, current_wave: u8 },
    Fusion { variants: Vec<String> },
}

pub struct AgentHierarchy {
    queen: AgentNode,
    planners: Vec<PlannerNode>,
}

pub struct PlannerNode {
    agent: AgentNode,
    domain: String,
    workers: Vec<AgentNode>,
    reviewer: Option<AgentNode>,
    tester: Option<AgentNode>,
}
```

#### File Watcher (`src-tauri/src/watcher/`)

```rust
pub struct SessionWatcher {
    watchers: HashMap<String, RecommendedWatcher>,
}

// Watch paths:
// - .hive/sessions/{id}/coordination.log
// - .swarm/sessions/{id}/logs/*.log
// - .swarm/sessions/{id}/state/*.md
// - .swarm/sessions/{id}/tasks/**/*.md

pub enum WatchEvent {
    CoordinationMessage { from: String, message_type: String, content: String },
    AgentLogUpdate { agent_id: String, lines: Vec<String> },
    StateFileChanged { file: String, content: String },
    TaskFileCreated { planner: String, task_file: String },
}
```

#### Agent Spawner (`src-tauri/src/spawner/`)

```rust
pub struct AgentSpawner;

impl AgentSpawner {
    pub fn spawn_claude(
        &self,
        model: &str,
        prompt_file: &Path,
        cwd: &Path,
    ) -> Result<PtySession>;

    pub fn spawn_wsl_agent(
        &self,
        prompt_file: &Path,
        cwd: &Path,
    ) -> Result<PtySession>;

    pub fn spawn_gemini(
        &self,
        model: &str,
        prompt_file: &Path,
        cwd: &Path,
    ) -> Result<PtySession>;

    pub fn spawn_opencode(
        &self,
        model: &str,
        prompt_file: &Path,
        cwd: &Path,
    ) -> Result<PtySession>;

    pub fn spawn_codex(
        &self,
        model: &str,
        prompt_file: &Path,
        cwd: &Path,
    ) -> Result<PtySession>;
}
```

### Frontend Components

#### Terminal Component (`src/lib/components/Terminal.svelte`)
- Wraps xterm.js
- Receives output events from Rust backend
- Sends input to Rust backend
- Displays status badge overlay

#### Hierarchy Tree (`src/lib/components/HierarchyTree.svelte`)
- Renders Queen → Planner → Worker tree
- Status indicators per node
- Click to focus terminal
- Expandable/collapsible

#### Coordination Log (`src/lib/components/CoordinationLog.svelte`)
- Parses coordination.log
- Colored by agent (Queen=purple, Planner=blue, etc.)
- Auto-scroll with pause

#### File Ownership Matrix (`src/lib/components/OwnershipMatrix.svelte`)
- Parses file-ownership.md
- Visual grid of files × planners
- Highlights conflicts

### Data Flow

#### Spawning an Agent
```
1. User clicks "Launch Swarm"
2. Frontend → Tauri command: launch_swarm(project, planner_count)
3. Rust creates session directory structure
4. Rust copies templates, generates prompts
5. Rust spawns Queen PTY
6. Rust returns session_id to frontend
7. Frontend subscribes to session events
8. Queen output streams via Tauri events
9. xterm.js renders output
```

#### Agent Output Detection
```
1. PTY reader receives bytes
2. Rust buffers and scans for patterns:
   - "?" at line end → WaitingForInput
   - "PLANNER_COMPLETE" → status update
   - Log format "[HH:MM:SS] AGENT:" → parse for coordination
3. Rust emits typed events to frontend
4. Frontend updates hierarchy status
5. If WaitingForInput → trigger alert
```

#### User Input
```
1. User types in xterm.js
2. xterm.js onData fires
3. Frontend → Tauri command: write_to_agent(agent_id, bytes)
4. Rust writes bytes to PTY stdin
5. Agent receives input
```

---

## UI/UX Specifications

### Main Layout

```
┌──────────────────────────────────────────────────────────────────────────┐
│  [≡] Hive Manager                                        [_] [□] [×]     │
├────────┬───────────────────────────────────────────────────┬─────────────┤
│        │  ┌─────────────────────┐ ┌─────────────────────┐  │             │
│ ACTIVE │  │ Queen           [█] │ │ Planner A       [█] │  │  HIERARCHY  │
│        │  │                     │ │                     │  │             │
│ > Swarm│  │ Phase 2: Spawning   │ │ Domain: Backend     │  │  Queen █    │
│   auth │  │ planners...         │ │ Spawning Worker 1a  │  │  ├─ Pa █    │
│        │  │                     │ │                     │  │  │  ├─W1a ⏳│
│        │  └─────────────────────┘ └─────────────────────┘  │  │  ├─W2a   │
│ RECENT │  ┌─────────────────────┐ ┌─────────────────────┐  │  │  └─W3a   │
│        │  │ Worker 1a       [⏳] │ │ Worker 2a       [░] │  │  └─ Pb ░    │
│ - hive │  │                     │ │                     │  │             │
│ - swarm│  │ Reading task...     │ │ (not started)       │  │ ───────────│
│        │  │                     │ │                     │  │ COORDINATION│
│        │  └─────────────────────┘ └─────────────────────┘  │ Pa: [STATUS]│
│ [+] New│                                                   │ Starting w1a│
├────────┴───────────────────────────────────────────────────┴─────────────┤
│ Type: [Swarm ▼]  Planners: [2][3][4]  Project: D:/Code/myapp  [Launch]   │
└──────────────────────────────────────────────────────────────────────────┘
```

### Status Badges

| Badge | Meaning | Color |
|-------|---------|-------|
| █ | Running | Blue `#7aa2f7` |
| ⏳ | Waiting for input | Yellow `#e0af68` |
| ✓ | Completed | Green `#9ece6a` |
| ✗ | Error | Red `#f7768e` |
| ░ | Not started | Gray `#414868` |
| ◐ | Starting | Cyan `#7dcfff` |

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+1-9` | Focus terminal by position |
| `Ctrl+Q` | Focus Queen |
| `Ctrl+A/B/C/D` | Focus Planner A/B/C/D |
| `Ctrl+Tab` | Cycle terminals |
| `Ctrl+Shift+Tab` | Cycle reverse |
| `Ctrl+N` | New session dialog |
| `Ctrl+W` | Stop focused agent |
| `Ctrl+Shift+W` | Stop entire session |
| `F11` | Toggle fullscreen terminal |
| `Ctrl+G` | Toggle grid/tree layout |
| `Ctrl+L` | Toggle coordination log panel |
| `Ctrl+F` | Search in focused terminal |
| `Ctrl+,` | Settings |
| `Escape` | Exit fullscreen / close dialog |

### Color Scheme (Tokyo Night)

| Element | Color |
|---------|-------|
| Background | `#1a1b26` |
| Surface | `#24283b` |
| Surface Hover | `#292e42` |
| Border | `#414868` |
| Text | `#c0caf5` |
| Text Muted | `#565f89` |
| Accent (Blue) | `#7aa2f7` |
| Success (Green) | `#9ece6a` |
| Warning (Yellow) | `#e0af68` |
| Error (Red) | `#f7768e` |
| Info (Cyan) | `#7dcfff` |
| Purple | `#bb9af7` |

### Agent Colors (for hierarchy/logs)

| Agent | Color |
|-------|-------|
| Queen | Purple `#bb9af7` |
| Planner | Blue `#7aa2f7` |
| Worker (Backend) | Cyan `#7dcfff` |
| Worker (Frontend) | Green `#9ece6a` |
| Worker (Coherence) | Yellow `#e0af68` |
| Worker (Simplify) | Orange `#ff9e64` |
| Reviewer | Magenta `#ff007c` |
| Tester | Teal `#1abc9c` |

---

## Update Strategy

### Distribution

| Component | Location |
|-----------|----------|
| Releases | GitHub Releases (private repo) |
| Manifest | `https://github.com/{user}/hive-manager/releases/latest/download/latest.json` |
| Installer | `.msi` (Windows) |

### Tauri Updater Config

```json
{
  "plugins": {
    "updater": {
      "active": true,
      "dialog": true,
      "endpoints": [
        "https://github.com/{user}/hive-manager/releases/latest/download/latest.json"
      ],
      "pubkey": "YOUR_PUBLIC_KEY"
    }
  }
}
```

### Update Flow

```
App Start → Check Updates → Available?
                              │
              ┌───────────────┴───────────────┐
              │ Yes                           │ No
              ▼                               ▼
    Show Changelog Dialog              Continue Normal
              │
    ┌─────────┴─────────┐
    │ Now         Later │
    ▼                   ▼
Download in BG    Remind Next Launch
Install on Exit
```

### Rollback

1. Previous installer saved to `%APPDATA%/hive-manager/backups/`
2. Settings stored separately (never overwritten)
3. "Rollback" in Settings → restores previous version

---

## Project Structure

```
D:/Code Projects/hive-manager/
├── src/                              # Svelte frontend
│   ├── lib/
│   │   ├── components/
│   │   │   ├── Terminal.svelte       # xterm.js wrapper
│   │   │   ├── TerminalGrid.svelte   # Grid layout manager
│   │   │   ├── HierarchyTree.svelte  # Agent hierarchy view
│   │   │   ├── CoordinationLog.svelte
│   │   │   ├── OwnershipMatrix.svelte
│   │   │   ├── SessionSidebar.svelte
│   │   │   ├── LaunchDialog.svelte
│   │   │   ├── SettingsDialog.svelte
│   │   │   └── StatusBadge.svelte
│   │   ├── stores/
│   │   │   ├── sessions.ts           # Session state
│   │   │   ├── agents.ts             # Agent hierarchy state
│   │   │   ├── terminals.ts          # Terminal instances
│   │   │   └── settings.ts           # User preferences
│   │   └── utils/
│   │       ├── tauri.ts              # Tauri IPC wrappers
│   │       └── terminal.ts           # xterm.js helpers
│   ├── routes/
│   │   └── +page.svelte
│   ├── app.html
│   └── app.css
├── src-tauri/                        # Rust backend
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs
│   │   ├── pty/
│   │   │   ├── mod.rs
│   │   │   ├── manager.rs
│   │   │   └── session.rs
│   │   ├── session/
│   │   │   ├── mod.rs
│   │   │   ├── manager.rs
│   │   │   ├── hive.rs
│   │   │   ├── swarm.rs
│   │   │   └── fusion.rs
│   │   ├── spawner/
│   │   │   ├── mod.rs
│   │   │   ├── claude.rs
│   │   │   ├── cursor.rs
│   │   │   ├── gemini.rs
│   │   │   ├── opencode.rs
│   │   │   └── codex.rs
│   │   ├── watcher/
│   │   │   ├── mod.rs
│   │   │   └── parser.rs
│   │   ├── updater/
│   │   │   └── mod.rs
│   │   ├── commands/
│   │   │   ├── mod.rs
│   │   │   ├── session.rs
│   │   │   ├── agent.rs
│   │   │   └── settings.rs
│   │   └── config/
│   │       └── mod.rs
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   └── icons/
├── static/
├── package.json
├── svelte.config.js
├── vite.config.ts
├── tsconfig.json
├── PRD.md
└── CLAUDE.md
```

---

## Dependencies

### Rust (Cargo.toml)

```toml
[dependencies]
tauri = { version = "2", features = ["shell-open"] }
tauri-plugin-updater = "2"
tauri-plugin-dialog = "2"
tauri-plugin-fs = "2"
tauri-plugin-shell = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
portable-pty = "0.8"
notify = "6"
notify-debouncer-mini = "0.4"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }
thiserror = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
directories = "5"
regex = "1"
```

### Node (package.json)

```json
{
  "devDependencies": {
    "@sveltejs/adapter-static": "^3",
    "@sveltejs/kit": "^2",
    "@sveltejs/vite-plugin-svelte": "^4",
    "@tauri-apps/cli": "^2",
    "svelte": "^5",
    "typescript": "^5",
    "vite": "^6"
  },
  "dependencies": {
    "@tauri-apps/api": "^2",
    "@tauri-apps/plugin-updater": "^2",
    "@tauri-apps/plugin-dialog": "^2",
    "@xterm/xterm": "^5",
    "@xterm/addon-fit": "^0.10",
    "@xterm/addon-webgl": "^0.18",
    "@xterm/addon-search": "^0.15"
  }
}
```

---

## Milestones

### M1: Foundation
- [ ] Tauri + Svelte project scaffold
- [ ] Single PTY terminal (claude CLI)
- [ ] Basic terminal input/output
- [ ] Minimal UI shell

### M2: Multi-CLI Support
- [ ] WSL PTY for agent CLI
- [ ] gemini CLI spawning
- [ ] opencode CLI spawning
- [ ] codex CLI spawning
- [ ] Agent type detection

### M3: Hive Sessions
- [ ] Hive session launch
- [ ] Queen + Workers hierarchy
- [ ] Basic hierarchy tree view
- [ ] Session sidebar

### M4: Swarm Sessions
- [ ] Swarm session launch
- [ ] Planner prompt generation
- [ ] File ownership parsing
- [ ] Coordination log viewer

### M5: Polish
- [ ] Input detection + alerts
- [ ] Keyboard shortcuts
- [ ] Layout options (grid/tree/tabs)
- [ ] Settings persistence
- [ ] Theme support

### M6: Distribution
- [ ] GitHub releases setup
- [ ] Tauri updater integration
- [ ] Auto-update flow
- [ ] MSI installer generation

---

## Future Considerations (v2+)

### A2A Protocol
- Replace file-based coordination with real-time messaging
- MCP server embedded in app
- Structured agent communication

### Analytics
- Token usage per agent
- Session duration metrics
- Success/failure tracking
- Cost estimation

### Remote Agents
- SSH tunnel for remote PTY
- Multi-machine coordination

---

## Appendix: Session Directory Structures

### Hive
```
.hive/sessions/{SESSION_ID}/
├── session.json
├── coordination.log
├── queen-prompt.md
├── tasks/
│   ├── worker-1-task.md
│   └── worker-2-task.md
├── state/
│   └── current.json
└── spawn/
    ├── queen.bat
    └── worker-*.bat
```

### Swarm
```
.swarm/sessions/{SESSION_ID}/
├── docs/
│   ├── model-selection.md
│   ├── spawn-templates.md
│   └── log-protocol.md
├── phases/
│   ├── phase-1-planning.md
│   ├── phase-2-execution.md
│   ├── phase-3-review.md
│   ├── phase-4-integration.md
│   └── phase-5-commit.md
├── state/
│   ├── context.md
│   ├── responsibility-matrix.md
│   ├── file-ownership.md
│   ├── session-guidelines.md
│   └── tasks.json
├── tasks/
│   ├── planner-a/
│   │   ├── worker-1a-task.md
│   │   └── worker-2a-task.md
│   └── planner-b/
│       └── ...
├── logs/
│   ├── queen.log
│   ├── coordination.log
│   ├── planner-a.log
│   └── planner-b.log
├── spawn/
│   ├── queen.bat
│   ├── planner-a.bat
│   ├── worker-1a.bat
│   └── ...
├── queen-prompt.md
├── planner-a-prompt.md
├── planner-b-prompt.md
└── launch.ps1
```

---

*Last updated: 2026-02-03*
