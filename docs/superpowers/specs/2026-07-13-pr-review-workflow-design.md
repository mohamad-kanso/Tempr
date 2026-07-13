# PR Review-Based Development Workflow

**Date:** 2026-07-13  
**Status:** Approved  
**Scope:** All feature-level work in Tempr (Phase checklist items)

## Problem

Agents working directly on `main` bypass review. Mistakes land in the canonical branch with no gate. The project needs a lightweight but enforced workflow that applies equally whether a human or an agent is doing the work.

## Decision

Approach A: CLAUDE.md hard rule + local git push protection.

- Documented as a mandatory rule in CLAUDE.md — same weight as "no business logic in UI"
- Local `pre-push` hook blocks any direct push to `main`
- No additional Claude Code hooks or skill wrappers needed

## Workflow

Every Phase checklist item follows this sequence:

```
feature-dev skill
  → git checkout -b feat/ph<N>-<short-description>
  → implement
  → /code-review
  → gh pr create
  → user approves
  → gh pr merge
```

No exceptions. Docs-only or config-only changes may use `docs/` or `chore/` branch prefixes but still go through a PR.

## Branch Naming

```
feat/ph<N>-<short-description>   # feature work
docs/<short-description>          # docs-only changes
chore/<short-description>         # tooling/config
fix/<short-description>           # bug fixes
```

## CLAUDE.md Change

Add to the **Hard rules** block:

> **Feature work follows branch → PR → review → merge.** Every Phase checklist item goes through: `feature-dev` skill → `git checkout -b feat/ph<N>-<slug>` → implement → `/code-review` → `gh pr create` → user approves → `gh pr merge`. No direct commits to main for feature work.

Add to the **Commands** block:

```bash
bash scripts/setup.sh   # install git hooks (run once after clone)
```

## Git Hook

Tracked at `.github/hooks/pre-push`. Installed locally by `scripts/setup.sh`.

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

## Setup Script

`scripts/setup.sh` — run once after clone:

```bash
#!/usr/bin/env bash
cp .github/hooks/pre-push .git/hooks/pre-push
chmod +x .git/hooks/pre-push
echo "Git hooks installed."
```

## Review Gate

Before `gh pr create`, `/code-review` must run on the branch diff. User then reviews the PR on GitHub and approves before merge. Both gates must pass.

## What This Does NOT Change

- Trivial doc corrections in an existing session may still commit directly if the change is one line and no behavior changes. Use judgment.
- CI already runs on every push — this workflow adds a PR gate on top, not instead of CI.
- GitHub branch protection rules (server-side) are optional at this stage; local hook is sufficient for now.

## Out of Scope

- Claude Code `PreToolUse`/`PostToolUse` hooks blocking commits (Approach B/C — not chosen)
- Wrapping `feature-dev` in a new skill
- Automated PR assignment or labeling
