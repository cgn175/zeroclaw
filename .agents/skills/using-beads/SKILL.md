---
name: using-beads
description: "Manages issues with the bd (Beads) CLI for in-repo issue tracking. Use when creating, updating, closing, or querying issues, or when starting/ending a work session."
---

# Beads Issue Tracking

This project uses **Beads** (`bd` CLI) for issue tracking. Issues live in `.beads/` alongside the code. The database is already initialized.

## Core Commands

### Find Work

```bash
bd ready                    # Show tasks with no open blockers
bd list                     # All issues
bd list --status open       # Filter by status (open, in_progress, closed, blocked)
bd list --type bug          # Filter by type (bug, feature, task)
bd list --priority 1        # Filter by priority (0=critical, 1=high, 2=medium, 3=low, 4=none)
bd show <id>                # Full issue details
```

### Create Issues

```bash
bd create "Title goes here"                              # Basic
bd create "Fix memory leak" -t bug -p 1                  # With type and priority
bd create "Refactor auth" -d "Description here" --json   # With description, JSON output
bd create "Subtask" --parent <parent-id>                 # Child of an epic/parent
```

**Rules:**
- Always wrap titles and descriptions in double quotes
- Use `--json` when you need to parse the output (extracts issue ID)
- Supported types: `bug`, `feature`, `task`
- Priority: 0 (critical) → 4 (none)

### Update Issues

```bash
bd update <id> --status in_progress    # Change status
bd update <id> --status blocked        # Mark blocked
bd update <id> --status closed         # Close via update
bd update <id> --claim                 # Atomically set assignee + in_progress
bd update <id> --priority 1            # Change priority
bd update <id> --title "New title"     # Change title
bd update <id> --description "New desc"
bd update <id> --design "Design notes"
bd update <id> --notes "Implementation notes"
bd update <id> --acceptance "Acceptance criteria"
```

**NEVER use `bd edit`** — it opens an interactive `$EDITOR` which does not work for agents.

### Close Issues

```bash
bd close <id>              # Mark as done
bd close <id> --json       # Close with JSON output
```

### Search and Query

```bash
bd search "query string"   # Full-text search
bd query "status:open AND priority:<2"  # Structured query
bd comments <id>           # View comments on an issue
bd blocked                 # Show all blocked issues
```

### Dependencies

```bash
bd dep add <issue> <depends-on>                # issue depends on (is blocked by) depends-on
bd dep <blocker-id> --blocks <blocked-id>      # Shorthand: blocker blocks blocked
bd dep relate <id-a> <id-b>                    # Bidirectional relates_to link
bd dep list <id>                               # List dependencies of an issue
bd dep tree <id>                               # Show dependency tree
```

### Sync with Git

```bash
bd sync    # Export → commit → pull → import → push (all-in-one)
```

Always run `bd sync` at session end. It bypasses the 30-second debounce and forces an immediate flush/commit/push.

## Workflows

### Starting a Session

1. Run `bd ready` to find available work
2. Pick a task and review it with `bd show <id>`
3. Claim it: `bd update <id> --claim`
4. Do the work

### During Work

- Create child issues for subtasks: `bd create "Subtask" --parent <id>`
- Add blockers: `bd dep <blocker-id> --blocks <blocked-id>`
- Update status as work progresses: `bd update <id> --status in_progress`
- Include issue ID in commit messages: `git commit -m "Fix the bug (bd-abc)"`

### Landing the Plane (Session End) — MANDATORY

**Work is NOT complete until `git push` succeeds.** Follow every step:

1. **File follow-up issues** — Create issues for anything unfinished
2. **Run quality gates** — Tests, linters, builds (if code changed)
3. **Update issue status** — Close finished work, update in-progress items
4. **Push to remote** — This is MANDATORY:
   ```bash
   git add -A
   git commit -m "Description of changes (bd-xxx)"
   git pull --rebase
   bd sync
   git push
   git status   # MUST show "up to date with origin"
   ```
5. **Clean up** — `git stash clear`, `git remote prune origin`
6. **Verify** — All changes committed AND pushed
7. **Hand off** — Summarize what was done and what's next

**CRITICAL:** Never stop before pushing. Never say "ready to push when you are." YOU must push.

## Testing

**Never pollute the production database with test issues.** Use a temporary database:

```bash
BEADS_DB=/tmp/test-beads.db bd create "Test issue"
BEADS_DB=/tmp/test-beads.db bd list
```

## Sandbox Detection

In sandboxed environments (like Amp), `bd` auto-detects and enables `--no-daemon`, `--no-auto-flush`, and `--no-auto-import`. No manual configuration needed.

## Tips

- Use `--json` flag on commands when you need machine-readable output
- Issue IDs use the repo prefix (auto-detected from directory name)
- Hierarchical IDs exist for epics: parent `bd-a3f8` → child `bd-a3f8.1`
- Use `gh` CLI (not browser) for GitHub issue/PR cross-referencing
- The JSONL file (`.beads/issues.jsonl`) is the portable, git-friendly format; the SQLite DB is local cache
