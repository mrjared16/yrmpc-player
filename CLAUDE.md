# LLM Agent Guidelines - yrmpc

**Project**: YouTube Music TUI Client (Rust + Ratatui)
**Updated**: 2025-12-22
**Status**: ✅ Core Playable - Daily Use

---

## 🚀 Quick Start

```bash
cd rmpc && cargo build              # Debug build (fast iteration)
../restart_daemon.sh                # Start daemon (uses ytx extractor)
./target/debug/rmpc                 # Start TUI
```

**Alternative** (debug daemon with verbose logging):
```bash
../restart_daemon_debug.sh          # Verbose logging to /tmp/rmpcd-debug.log
```

**Extractor options** (in restart scripts):
- `--extractor ytx` - Fast (~200ms), requires `ytx` binary in PATH
- `--extractor ytdlp` - Reliable (~3-4s), default fallback

---

## 🔨 Build Guidelines

> ⚠️ **CRITICAL**: Use debug builds during development!

- `cargo build` - Debug (~45s) - Use for development and testing
- `cargo build --release` - Release (~3-4 min) - Only after user confirms all fixed

**Rule**: NEVER use release build just to check compilation. Debug catches same errors faster.

---

## 📋 Task Management (backlog.md CLI)

> ⚠️ **CRITICAL**: This project uses `backlog` CLI for task management. **Do NOT edit task files directly.**

### View Tasks
```bash
backlog task list --plain           # List all tasks
backlog task 1 --plain              # View specific task
backlog search "queue" --plain      # Search tasks
```

### Work on a Task
```bash
# 1. Assign and start
backlog task edit <id> -s "In Progress" -a @agent

# 2. Add implementation plan
backlog task edit <id> --plan "1. Research\n2. Implement\n3. Test"

# 3. After coding, mark AC complete
backlog task edit <id> --check-ac 1 --check-ac 2

# 4. Add notes and complete
backlog task edit <id> --notes "Implemented X"
backlog task edit <id> -s Done
```

> **Full guide**: [BACKLOG_INSTRUCTIONS.md](BACKLOG_INSTRUCTIONS.md)

---

## 📖 Documentation Index

| Purpose | File |
|---------|------|
| **This file** | Entry point for LLM agents |
| **Project vision** | [docs/VISION.md](docs/VISION.md) |
| **User workflow guide** | [docs/USER_GUIDE.md](docs/USER_GUIDE.md) |
| Task management | [BACKLOG_INSTRUCTIONS.md](BACKLOG_INSTRUCTIONS.md) |
| Project overview | [docs/PROJECT_OVERVIEW.md](docs/PROJECT_OVERVIEW.md) |
| Architecture | [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) |
| UI/UX spec | [docs/ui-ux-provised.md](docs/ui-ux-provised.md) |
| Rich List UI | [docs/arch/ui-navigation.md](docs/arch/ui-navigation.md) |
| YouTube API | [docs/YOUTUBE_API.md](docs/YOUTUBE_API.md) |
| Project goals | [docs/VISION.md](docs/VISION.md) |

---

## 🎯 Current State

| Feature | Status |
|---------|--------|
| Search (all types) | ✅ |
| Playback (MPV) | ✅ |
| Queue management | ✅ |
| Auto-advance | ✅ |
| Repeat (One/All) | ✅ |
| Shuffle | ✅ |
| MPRIS integration | ✅ |
| Daemon mode | ✅ |
| Rich List UI | ✅ |

---

## 📁 Key Files

| Purpose | Path |
|---------|------|
| YouTube API | `rmpc/src/player/youtube/api.rs` |
| Protocol | `rmpc/src/player/youtube/protocol.rs` |
| Rich List Widget | `rmpc/src/ui/widgets/item_list.rs` |
| Display Trait | `rmpc/src/domain/display.rs` |
| Search Pane | `rmpc/src/ui/panes/search/mod.rs` |
| Queue Pane | `rmpc/src/ui/panes/queue.rs` |

---

## ⚡ Session Workflow

### Starting a Session

1. **Check backlog**: `backlog task list --plain`
2. **Pick a task** or discuss with user
3. **Claim it**: `backlog task edit <id> -s "In Progress" -a @agent`
4. **Add plan**: `backlog task edit <id> --plan "..."`
5. **Get approval** before coding

### During Implementation

- Update AC as you complete: `backlog task edit <id> --check-ac 1`
- Append notes: `backlog task edit <id> --append-notes "Progress..."`

### Finishing

1. Check all AC: `backlog task <id> --plain`
2. Add final notes
3. Mark done: `backlog task edit <id> -s Done`

---

## 🧠 Guidelines

- **Read vision first**: Understand project goals in [docs/VISION.md](docs/VISION.md)
- **Know user workflow**: Study [docs/USER_GUIDE.md](docs/USER_GUIDE.md) for daily usage patterns
- **Think first**: Use sequential thinking for complex problems
- **Research before fixing**: Find root cause, don't guess
- **TDD when possible**: Tests prove correctness
- **Ask questions**: Clarify assumptions with user
- **Document changes**: Update session context files
- **Expert thinking**: 
```
Think harder. Critique before responding.

You're a principal engineer (10+ yrs production streaming). Apply SOLID, prioritize maintainability, refactor proactively when spotting better designs.

**Each phase**: "Best approach? What breaks at scale?"  
**Show reasoning** when pivoting from initial solution.
```


---

## ⚠️ Critical Rules

### NEVER
- Edit backlog task files directly (use CLI only)
- Download entire playlists (stream only)
- Break vim-style keyboard navigation

### ALWAYS
- Use `--plain` flag when viewing backlog tasks
- Test with real YouTube content
- Update task AC as you complete them
- Keep user informed of blockers
- Read VISION.md and USER_GUIDE.md before starting any task
