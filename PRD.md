# Hive Manager - Product Requirements Document

## Overview

**Product Name:** Hive Manager
**Version:** 1.0.0
**Platform:** Windows (primary), macOS/Linux (future)
**Tech Stack:** Tauri v2 + Rust backend + Svelte frontend

### Vision

A native desktop application for orchestrating and monitoring Claude Code multi-agent workflows (Hives, Swarms, and Fusions). Provides embedded terminals with full interactivity, allowing users to monitor, steer, and respond to agent prompts from a unified interface.

### Problem Statement

Currently, multi-agent workflows spawn multiple Windows Terminal tabs. Users must:
- Manually track which tab belongs to which agent
- Switch between tabs to monitor progress
- Risk missing agent prompts buried in background tabs
- Have no unified view of session state

### Solution

A single-window application with:
- Embedded terminal panels for each agent
- Real-time session state visualization
- Unified prompt/input handling
- Session history and log access

---

## User Personas

### Primary: Power User (You)
- Runs complex multi-agent workflows daily
- Needs to monitor 2-10+ agents simultaneously
- Requires ability to intervene/steer agents mid-task
- Values efficiency and keyboard-driven workflows

### Secondary: Team Member (Future)
- Occasional multi-agent usage
- Prefers visual overview over terminal diving
- May not understand agent internals

---

## Functional Requirements

### F1: Session Management

| ID | Requirement | Priority |
|----|-------------|----------|
| F1.1 | Launch new Hive session (1-4 workers) | P0 |
| F1.2 | Launch new Swarm session (Queen + Planners + Workers) | P0 |
| F1.3 | Launch new Fusion session (competing implementations) | P1 |
| F1.4 | Stop/terminate individual agents | P0 |
| F1.5 | Stop/terminate entire session | P0 |
| F1.6 | Pause/resume agent (if supported by CLI) | P2 |
| F1.7 | View historical sessions | P1 |
| F1.8 | Resume interrupted session | P2 |

### F2: Embedded Terminals

| ID | Requirement | Priority |
|----|-------------|----------|
| F2.1 | Render agent output in real-time via xterm.js | P0 |
| F2.2 | Send keyboard input to agent | P0 |
| F2.3 | Support ANSI colors and formatting | P0 |
| F2.4 | Auto-scroll with lock/unlock option | P0 |
| F2.5 | Search within terminal output | P1 |
| F2.6 | Copy/paste support | P0 |
| F2.7 | Terminal resize (responsive panels) | P0 |
| F2.8 | Multiple layout options (tabs, grid, split) | P1 |

### F3: Session Monitoring

| ID | Requirement | Priority |
|----|-------------|----------|
| F3.1 | Display session hierarchy (Queen → Planners → Workers) | P0 |
| F3.2 | Show agent status (running, waiting, completed, error) | P0 |
| F3.3 | Parse and display task assignments | P1 |
| F3.4 | Show file ownership matrix (Swarm) | P1 |
| F3.5 | Display coordination.log in formatted view | P1 |
| F3.6 | Alert when agent requests input | P0 |
| F3.7 | Show token/cost usage per agent (if available) | P2 |

### F4: Agent Interaction

| ID | Requirement | Priority |
|----|-------------|----------|
| F4.1 | Focus on agent requesting input | P0 |
| F4.2 | Send text input to agent | P0 |
| F4.3 | Send common responses (y/n, approve, reject) via hotkey | P1 |
| F4.4 | Broadcast message to all agents in session | P2 |
| F4.5 | Inject context/instructions mid-session | P2 |

### F5: Configuration

| ID | Requirement | Priority |
|----|-------------|----------|
| F5.1 | Configure default worker count | P1 |
| F5.2 | Configure default models per agent role | P1 |
| F5.3 | Set session directory paths | P1 |
| F5.4 | Configure keyboard shortcuts | P1 |
| F5.5 | Theme selection (dark/light) | P2 |

### F6: Updates & Maintenance

| ID | Requirement | Priority |
|----|-------------|----------|
| F6.1 | Check for updates on startup | P0 |
| F6.2 | Download and apply updates automatically | P1 |
| F6.3 | Manual update check via menu | P0 |
| F6.4 | Show changelog before update | P1 |
| F6.5 | Rollback to previous version | P2 |
| F6.6 | Update without losing active sessions | P2 |

---

## Technical Architecture

### System Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        HIVE MANAGER                             │
├─────────────────────────────────────────────────────────────────┤
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                    SVELTE FRONTEND                        │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌─────────────────┐   │  │
│  │  │  Sidebar    │  │  Terminal   │  │  Status Panel   │   │  │
│  │  │  - Sessions │  │  Grid/Tabs  │  │  - Hierarchy    │   │  │
│  │  │  - History  │  │  (xterm.js) │  │  - Tasks        │   │  │
│  │  │  - Settings │  │             │  │  - Alerts       │   │  │
│  │  └─────────────┘  └─────────────┘  └─────────────────┘   │  │
│  └───────────────────────────────────────────────────────────┘  │
│                              │                                   │
│                    Tauri IPC + Events                           │
│                              │                                   │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                     RUST BACKEND                          │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐  │  │
│  │  │ PTY Manager  │  │ Session      │  │ File Watcher   │  │  │
│  │  │ (portable-   │  │ Controller   │  │ (notify)       │  │  │
│  │  │  pty)        │  │              │  │                │  │  │
│  │  └──────────────┘  └──────────────┘  └────────────────┘  │  │
│  │  ┌──────────────┐  ┌──────────────┐  ┌────────────────┐  │  │
│  │  │ Update       │  │ Config       │  │ Log Parser     │  │  │
│  │  │ Manager      │  │ Store        │  │                │  │  │
│  │  └──────────────┘  └──────────────┘  └────────────────┘  │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
        ┌──────────┐   ┌──────────┐    ┌──────────┐
        │  Queen   │   │ Worker 1 │    │ Worker N │
        │  (PTY)   │   │  (PTY)   │    │  (PTY)   │
        └──────────┘   └──────────┘    └──────────┘
              │               │               │
              └───────────────┴───────────────┘
                    claude CLI processes
```

### Rust Backend Components

#### PTY Manager (`src-tauri/src/pty/`)
```rust
// Core responsibilities:
// - Spawn claude CLI with PTY
// - Manage PTY I/O streams
// - Handle resize events
// - Clean shutdown

pub struct PtySession {
    id: String,
    role: AgentRole,
    pty: Box<dyn PtyProcess>,
    reader: AsyncReader,
    writer: AsyncWriter,
    status: AgentStatus,
}

pub enum AgentRole {
    Queen,
    Planner { index: u8 },
    Worker { index: u8, parent: Option<String> },
    Fusion { variant: String },
}

pub enum AgentStatus {
    Starting,
    Running,
    WaitingForInput,
    Completed,
    Error(String),
}
```

#### Session Controller (`src-tauri/src/session/`)
```rust
// Core responsibilities:
// - Orchestrate multi-agent sessions
// - Track session state
// - Handle session lifecycle

pub struct Session {
    id: String,
    session_type: SessionType,
    project_path: PathBuf,
    agents: HashMap<String, PtySession>,
    state: SessionState,
    created_at: DateTime<Utc>,
}

pub enum SessionType {
    Hive { worker_count: u8 },
    Swarm { planner_count: u8 },
    Fusion { variants: Vec<String> },
}
```

#### File Watcher (`src-tauri/src/watcher/`)
```rust
// Core responsibilities:
// - Watch .hive/sessions/ and .swarm/sessions/
// - Parse coordination.log changes
// - Parse task file updates
// - Emit structured events to frontend
```

#### Update Manager (`src-tauri/src/updater/`)
```rust
// Core responsibilities:
// - Check GitHub releases for updates
// - Download update packages
// - Apply updates (Tauri updater plugin)
// - Manage rollback capability
```

### Frontend Components

#### Terminal Component (`src/lib/components/Terminal.svelte`)
```svelte
<!-- Wraps xterm.js -->
<!-- Props: agentId, onInput, onResize -->
<!-- Events: input, resize, focus -->
```

#### Session Sidebar (`src/lib/components/SessionSidebar.svelte`)
```svelte
<!-- Lists active and historical sessions -->
<!-- Expandable tree view for agents -->
<!-- Status indicators -->
```

#### Terminal Grid (`src/lib/components/TerminalGrid.svelte`)
```svelte
<!-- Manages terminal panel layout -->
<!-- Supports: tabs, 2x2 grid, horizontal split, vertical split -->
<!-- Drag-to-resize panels -->
```

#### Status Panel (`src/lib/components/StatusPanel.svelte`)
```svelte
<!-- Shows session hierarchy -->
<!-- Task assignments -->
<!-- File ownership (Swarm) -->
<!-- Input alerts -->
```

### Data Flow

#### Agent Output → Frontend
```
1. Claude writes to PTY stdout
2. Rust PtySession reads bytes
3. Rust emits Tauri event: { agent_id, data: bytes }
4. Frontend receives event
5. xterm.js writes bytes to terminal
```

#### User Input → Agent
```
1. User types in xterm.js
2. xterm.js onData callback fires
3. Frontend invokes Tauri command: write_to_agent(id, bytes)
4. Rust PtySession writes bytes to PTY stdin
5. Claude CLI receives input
```

#### Session File Changes → Frontend
```
1. Agent writes to .hive/sessions/{id}/tasks/
2. notify crate detects file change
3. Rust parses file content
4. Rust emits Tauri event: { session_id, event_type, data }
5. Frontend updates state store
6. UI reactively updates
```

---

## UI/UX Specifications

### Layout

```
┌──────────────────────────────────────────────────────────────────┐
│  [≡] Hive Manager                              [_] [□] [×]       │
├────────┬─────────────────────────────────────────────┬───────────┤
│        │  ┌─────────────────┐ ┌─────────────────┐    │           │
│ ACTIVE │  │ Queen           │ │ Worker 1        │    │  STATUS   │
│        │  │ █ Running       │ │ ⏳ Waiting      │    │           │
│ > Hive │  │                 │ │                 │    │ Hierarchy │
│   2024 │  │ $ claude...     │ │ Waiting for     │    │ ├─ Queen  │
│        │  │ > Analyzing...  │ │ task from Queen │    │ ├─ W1 ⏳  │
│        │  │                 │ │                 │    │ └─ W2 █   │
│ RECENT │  └─────────────────┘ └─────────────────┘    │           │
│        │  ┌─────────────────┐ ┌─────────────────┐    │ Tasks     │
│ - Swarm│  │ Worker 2        │ │ Worker 3        │    │ □ Task 1  │
│ - Hive │  │ █ Running       │ │ ✓ Complete      │    │ ■ Task 2  │
│        │  │                 │ │                 │    │ □ Task 3  │
│        │  │ Implementing... │ │ Done: auth.ts   │    │           │
│ [+] New│  │                 │ │                 │    │ ⚠ W1 needs│
│        │  └─────────────────┘ └─────────────────┘    │   input   │
├────────┴─────────────────────────────────────────────┴───────────┤
│ [Hive ▼] Workers: [2] [3] [4]  Model: [sonnet ▼]  [Launch]       │
└──────────────────────────────────────────────────────────────────┘
```

### Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl+1-9` | Focus terminal 1-9 |
| `Ctrl+Tab` | Cycle terminals |
| `Ctrl+Shift+Tab` | Cycle reverse |
| `Ctrl+N` | New session dialog |
| `Ctrl+W` | Close focused terminal |
| `Ctrl+Shift+W` | Stop entire session |
| `F11` | Toggle fullscreen terminal |
| `Ctrl+F` | Search in terminal |
| `Ctrl+,` | Open settings |
| `Ctrl+J` | Toggle status panel |

### Color Scheme (Dark Theme - Tokyo Night)

| Element | Color |
|---------|-------|
| Background | `#1a1b26` |
| Surface | `#24283b` |
| Border | `#414868` |
| Text | `#c0caf5` |
| Accent | `#7aa2f7` |
| Success | `#9ece6a` |
| Warning | `#e0af68` |
| Error | `#f7768e` |
| Running | `#7aa2f7` |
| Waiting | `#e0af68` |
| Complete | `#9ece6a` |

---

## Update Strategy

### Update Distribution

| Component | Location |
|-----------|----------|
| Releases | GitHub Releases |
| Update manifest | `https://github.com/{user}/hive-manager/releases/latest/download/latest.json` |
| Installers | `.msi` (Windows), `.dmg` (macOS), `.AppImage` (Linux) |

### Tauri Updater Configuration

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
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│ App Starts  │────▶│ Check for   │────▶│ Update      │
│             │     │ Updates     │     │ Available?  │
└─────────────┘     └─────────────┘     └──────┬──────┘
                                               │
                    ┌──────────────────────────┼──────────────────────┐
                    │ Yes                      │                  No  │
                    ▼                          │                      ▼
           ┌─────────────────┐                 │             ┌─────────────┐
           │ Show Changelog  │                 │             │ Continue    │
           │ Dialog          │                 │             │ Normal      │
           └────────┬────────┘                 │             └─────────────┘
                    │                          │
         ┌──────────┴──────────┐               │
         │ Update Now    Later │               │
         ▼                     ▼               │
┌─────────────────┐   ┌─────────────────┐      │
│ Download in BG  │   │ Remind Next     │      │
│ Install on Exit │   │ Launch          │      │
└─────────────────┘   └─────────────────┘      │
```

### Versioning

- **Semantic Versioning:** `MAJOR.MINOR.PATCH`
- **Major:** Breaking changes, major UI overhaul
- **Minor:** New features, non-breaking
- **Patch:** Bug fixes, minor improvements

### Release Channels (Future)

| Channel | Purpose |
|---------|---------|
| `stable` | Production releases |
| `beta` | Pre-release testing |
| `dev` | Nightly builds |

### Rollback Strategy

1. Keep previous version installer in `%APPDATA%/hive-manager/backups/`
2. Settings stored separately from app binary
3. "Rollback" option in settings restores previous version
4. Session data never modified by updates

---

## Project Structure

```
D:/Code Projects/hive-manager/
├── src/                          # Svelte frontend
│   ├── lib/
│   │   ├── components/
│   │   │   ├── Terminal.svelte
│   │   │   ├── TerminalGrid.svelte
│   │   │   ├── SessionSidebar.svelte
│   │   │   ├── StatusPanel.svelte
│   │   │   ├── LaunchDialog.svelte
│   │   │   └── SettingsDialog.svelte
│   │   ├── stores/
│   │   │   ├── sessions.ts
│   │   │   ├── terminals.ts
│   │   │   └── settings.ts
│   │   └── utils/
│   │       ├── tauri.ts
│   │       └── terminal.ts
│   ├── routes/
│   │   └── +page.svelte
│   ├── app.html
│   └── app.css
├── src-tauri/                    # Rust backend
│   ├── src/
│   │   ├── main.rs
│   │   ├── lib.rs
│   │   ├── pty/
│   │   │   ├── mod.rs
│   │   │   ├── manager.rs
│   │   │   └── session.rs
│   │   ├── session/
│   │   │   ├── mod.rs
│   │   │   ├── controller.rs
│   │   │   ├── hive.rs
│   │   │   ├── swarm.rs
│   │   │   └── fusion.rs
│   │   ├── watcher/
│   │   │   ├── mod.rs
│   │   │   └── parser.rs
│   │   ├── updater/
│   │   │   └── mod.rs
│   │   ├── commands/
│   │   │   ├── mod.rs
│   │   │   ├── session.rs
│   │   │   ├── terminal.rs
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
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
portable-pty = "0.8"
notify = "6"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1", features = ["v4"] }
thiserror = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
directories = "5"
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
    "@xterm/xterm": "^5",
    "@xterm/addon-fit": "^0.10",
    "@xterm/addon-webgl": "^0.18",
    "@xterm/addon-search": "^0.15"
  }
}
```

---

## Milestones

### M1: Foundation (MVP)
- [ ] Tauri + Svelte project scaffold
- [ ] Single PTY terminal working
- [ ] Basic session launch (Hive only)
- [ ] Terminal input/output functional
- [ ] Minimal UI (sidebar + single terminal)

### M2: Multi-Agent
- [ ] Multiple PTY sessions
- [ ] Terminal grid layout
- [ ] Session hierarchy display
- [ ] Agent status tracking
- [ ] Input alert notifications

### M3: Full Session Types
- [ ] Swarm session support
- [ ] Fusion session support
- [ ] File watcher integration
- [ ] Task parsing and display
- [ ] Coordination log viewer

### M4: Polish
- [ ] Keyboard shortcuts
- [ ] Settings persistence
- [ ] Theme support
- [ ] Terminal search
- [ ] Session history

### M5: Updates & Distribution
- [ ] GitHub releases setup
- [ ] Tauri updater integration
- [ ] Auto-update flow
- [ ] Installer generation
- [ ] Rollback capability

---

## Future Considerations (v2+)

### A2A Protocol Integration
- Replace file-based coordination with real-time messaging
- MCP server embedded in app
- Structured agent-to-agent communication
- Message history and replay

### Multi-Machine Support
- Remote agent monitoring
- SSH tunnel for remote PTY
- Centralized session management

### Analytics Dashboard
- Token usage tracking
- Session duration metrics
- Success/failure rates
- Cost estimation

### Plugin System
- Custom session types
- Third-party integrations
- Custom status parsers

---

## Open Questions

1. **Session persistence:** Should we save/restore terminal scroll history between app restarts?
2. **Multi-project:** Support multiple projects open simultaneously?
3. **Log export:** Export session logs to file?
4. **Notifications:** System tray notifications for agent prompts when app is minimized?
5. **Collaboration:** Share session view with another user? (v2+)

---

## Appendix: Session Directory Structures

### Hive Session
```
.hive/sessions/{SESSION_ID}/
├── session.json
├── coordination.log
├── tasks/
│   ├── task-001.json
│   └── task-002.json
└── state/
    └── current.json
```

### Swarm Session
```
.swarm/sessions/{SESSION_ID}/
├── docs/
│   ├── scope.md
│   └── architecture.md
├── phases/
│   ├── phase-1-planning.md
│   └── phase-2-execution.md
├── state/
│   ├── ownership-matrix.json
│   └── current.json
├── tasks/
└── logs/
    └── coordination.log
```

---

*Last updated: 2026-02-03*
