# PR Review-Based Development Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enforce branch → PR → review → merge for all feature work via a local git pre-push hook and a CLAUDE.md hard rule.

**Architecture:** Three deliverables: a tracked `pre-push` hook at `.github/hooks/pre-push`, a one-time setup script at `scripts/setup.sh` that installs it, and two additions to `CLAUDE.md` (hard rule + command). A DECISIONS.md entry (D13) records the decision.

**Tech Stack:** Bash (hooks + setup script), Markdown (CLAUDE.md, DECISIONS.md, PROGRESS.md)

## Global Constraints

- No new dependencies introduced
- Hook must be POSIX-compatible bash — no bashisms beyond `[[ ]]`
- `scripts/setup.sh` must be idempotent (safe to run multiple times)
- All commits use Conventional Commits format (`chore:`, `docs:`)
- DECISIONS.md is append-only — never edit existing entries

---

### Task 1: Create pre-push hook and setup script

**Files:**
- Create: `.github/hooks/pre-push`
- Create: `scripts/setup.sh`

**Interfaces:**
- Produces: `.github/hooks/pre-push` — a bash script that exits 1 when remote ref is `refs/heads/main`
- Produces: `scripts/setup.sh` — copies hook to `.git/hooks/pre-push` and makes it executable

- [ ] **Step 1: Create `.github/hooks/` directory and write hook**

```bash
mkdir -p /home/mohamad/Work/tempr/.github/hooks
```

Write `.github/hooks/pre-push` with this exact content:

```bash
#!/usr/bin/env bash
while read local_ref local_sha remote_ref remote_sha; do
  if [[ "$remote_ref" == "refs/heads/main" ]]; then
    echo "ERROR: Direct push to main blocked. Open a PR from a feat/* branch."
    exit 1
  fi
done
exit 0
```

Make it executable:

```bash
chmod +x /home/mohamad/Work/tempr/.github/hooks/pre-push
```

- [ ] **Step 2: Verify hook syntax**

```bash
bash -n /home/mohamad/Work/tempr/.github/hooks/pre-push
```

Expected: no output, exit 0.

- [ ] **Step 3: Create `scripts/` directory and write setup script**

Write `scripts/setup.sh` with this exact content:

```bash
#!/usr/bin/env bash
set -euo pipefail
cp .github/hooks/pre-push .git/hooks/pre-push
chmod +x .git/hooks/pre-push
echo "Git hooks installed."
```

Make it executable:

```bash
chmod +x /home/mohamad/Work/tempr/scripts/setup.sh
```

- [ ] **Step 4: Verify setup script syntax**

```bash
bash -n /home/mohamad/Work/tempr/scripts/setup.sh
```

Expected: no output, exit 0.

- [ ] **Step 5: Install the hook locally and verify**

```bash
cd /home/mohamad/Work/tempr && bash scripts/setup.sh
```

Expected output: `Git hooks installed.`

Then verify:

```bash
ls -la /home/mohamad/Work/tempr/.git/hooks/pre-push
```

Expected: file exists and is executable (`-rwxr-xr-x`).

- [ ] **Step 6: Functional test — hook blocks push to main**

Test the hook logic directly (no actual push needed):

```bash
echo "refs/heads/feat/test abc123 refs/heads/main abc456" | bash /home/mohamad/Work/tempr/.git/hooks/pre-push
```

Expected: prints `ERROR: Direct push to main blocked. Open a PR from a feat/* branch.` and exits 1.

```bash
echo "refs/heads/feat/test abc123 refs/heads/feat/ph1-postgres abc456" | bash /home/mohamad/Work/tempr/.git/hooks/pre-push
```

Expected: no output, exits 0.

- [ ] **Step 7: Commit**

```bash
git -C /home/mohamad/Work/tempr add .github/hooks/pre-push scripts/setup.sh
git -C /home/mohamad/Work/tempr commit -m "chore: add pre-push hook and setup script for branch protection"
```

---

### Task 2: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (Hard rules block ~line 94, Commands block ~line 84)

**Interfaces:**
- Consumes: nothing from Task 1
- Produces: CLAUDE.md with new hard rule and `bash scripts/setup.sh` command

- [ ] **Step 1: Add command to Commands block**

In `CLAUDE.md`, locate the Commands block (currently lines 86–92):

```markdown
```bash
cargo build                  # build
cargo fmt --check            # format check
cargo clippy -- -D warnings  # lint (treat warnings as errors)
cargo test                   # unit + integration tests
cargo deny check             # dependency audit (set up in Phase 0)
```
```

Replace with:

```markdown
```bash
cargo build                  # build
cargo fmt --check            # format check
cargo clippy -- -D warnings  # lint (treat warnings as errors)
cargo test                   # unit + integration tests
cargo deny check             # dependency audit (set up in Phase 0)
bash scripts/setup.sh        # install git hooks (run once after clone)
```
```

- [ ] **Step 2: Add hard rule to Hard rules block**

In `CLAUDE.md`, locate the Hard rules block. After the last rule (currently the Living docs rule), add:

```markdown
- **Feature work follows branch → PR → review → merge.** Every Phase checklist item goes through: `feature-dev` skill → `git checkout -b feat/ph<N>-<slug>` → implement → `/code-review` → `gh pr create` → user approves → `gh pr merge`. No direct commits to main for feature work. Branch prefixes: `feat/ph<N>-*`, `fix/*`, `docs/*`, `chore/*`. — see D13.
```

- [ ] **Step 3: Verify CLAUDE.md**

```bash
grep -n "scripts/setup.sh" /home/mohamad/Work/tempr/CLAUDE.md
grep -n "D13" /home/mohamad/Work/tempr/CLAUDE.md
```

Expected: both lines found.

- [ ] **Step 4: Commit**

```bash
git -C /home/mohamad/Work/tempr add CLAUDE.md
git -C /home/mohamad/Work/tempr commit -m "docs: enforce PR review workflow in CLAUDE.md hard rules"
```

---

### Task 3: Update DECISIONS.md and PROGRESS.md

**Files:**
- Modify: `docs/DECISIONS.md` (append D13 to index + full entry)
- Modify: `docs/PROGRESS.md` (decisions log row + session log row)

**Interfaces:**
- Consumes: D13 number (next after D12 in DECISIONS.md)
- Produces: DECISIONS.md with D13 entry; PROGRESS.md decisions log row linking `→ D13`

- [ ] **Step 1: Add D13 to DECISIONS.md index**

In `docs/DECISIONS.md`, locate the index table (ends with `| D12 | ... |`). Add:

```markdown
| D13 | 2026-07-13 | PR review-based development workflow — branch → PR → /code-review → user approval → merge | User |
```

- [ ] **Step 2: Append D13 full entry to DECISIONS.md**

At the end of `docs/DECISIONS.md`, append:

```markdown
---

## D13 — PR review-based development workflow (2026-07-13)

**By**: User.
**Decision**: All feature-level work (Phase checklist items) follows: `feature-dev` skill → `git checkout -b feat/ph<N>-<slug>` → implement → `/code-review` → `gh pr create` → user approves → `gh pr merge`. No direct pushes to `main`. Enforced locally by `.github/hooks/pre-push` (installed via `scripts/setup.sh`). Branch prefixes: `feat/ph<N>-*`, `fix/*`, `docs/*`, `chore/*`.
**Why**: Agents working directly on `main` bypass review. A lightweight push-gate plus a CLAUDE.md hard rule ensures both human and agent work goes through review before landing. Approach A (CLAUDE.md rule + local git hook) chosen over heavier Claude Code PreToolUse hooks for simplicity.
**Consequences**: Every feature branch requires a PR and a `/code-review` pass before merge. Trivial one-line doc corrections in the same session may still land directly — use judgment. GitHub server-side branch protection is optional at this stage.
```

- [ ] **Step 3: Add decisions log row to PROGRESS.md**

In `docs/PROGRESS.md`, locate the decisions log table. Add row:

```markdown
| 2026-07-13 | PR review-based workflow adopted — branch → PR → /code-review → user approval | → D13 |
```

- [ ] **Step 4: Update PROGRESS.md session log**

In the session log table, update the 2026-07-13 row to append to "What was done":

> `; PR review workflow (D13) adopted: pre-push hook + setup script + CLAUDE.md hard rule`

And append to "Follow-ups":

> `run bash scripts/setup.sh in each new worktree`

- [ ] **Step 5: Verify**

```bash
grep -n "D13" /home/mohamad/Work/tempr/docs/DECISIONS.md | head -5
grep -n "D13" /home/mohamad/Work/tempr/docs/PROGRESS.md
```

Expected: D13 appears in both files.

- [ ] **Step 6: Commit**

```bash
git -C /home/mohamad/Work/tempr add docs/DECISIONS.md docs/PROGRESS.md
git -C /home/mohamad/Work/tempr commit -m "docs: record D13 PR review workflow decision in living docs"
```
