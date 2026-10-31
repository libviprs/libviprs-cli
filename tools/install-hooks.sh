#!/usr/bin/env bash
# Point this clone (and its worktrees) at the tracked hooks in tools/hooks.
# core.hooksPath replaces .git/hooks, so a hook an older installer left there goes inert.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
chmod +x tools/gate.sh tools/hooks/pre-commit tools/hooks/pre-push
git config core.hooksPath tools/hooks
echo "hooks: core.hooksPath = tools/hooks (gates run on MARS, see tools/gate.sh)"
