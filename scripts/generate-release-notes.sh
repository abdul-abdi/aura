#!/usr/bin/env bash
# generate-release-notes.sh — Build beautiful release notes from conventional commits.
#
# Usage:
#   bash scripts/generate-release-notes.sh                     # current version, auto-detect base
#   bash scripts/generate-release-notes.sh --for v1.0.1        # generate notes for a specific release
#   bash scripts/generate-release-notes.sh v1.0.0              # current version, explicit base tag
#
# Reads commits between the base tag and target, groups them by conventional-commit
# type, and outputs formatted Markdown to stdout.
set -euo pipefail

# --- Parse arguments -----------------------------------------------------------

TARGET_TAG=""
BASE_TAG=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --for) TARGET_TAG="$2"; shift 2 ;;
    *)     BASE_TAG="$1"; shift ;;
  esac
done

# Determine version and target ref
if [[ -n "$TARGET_TAG" ]]; then
  VERSION="${TARGET_TAG#v}"
  TARGET_REF="$TARGET_TAG"
else
  VERSION=$(grep '^version' Cargo.toml | head -1 | sed 's/.*"\(.*\)"/\1/')
  TARGET_TAG="v${VERSION}"
  TARGET_REF="HEAD"
fi

# Auto-detect base tag: find the tag right before the target in semver order
if [[ -z "$BASE_TAG" ]]; then
  BASE_TAG=$(git tag -l 'v*' --sort=-version:refname \
    | grep -v "^${TARGET_TAG}$" \
    | while read -r tag; do
        # Only include tags that are ancestors of the target
        if git merge-base --is-ancestor "$tag" "$TARGET_REF" 2>/dev/null; then
          echo "$tag"
          break
        fi
      done || echo "")
fi

# Build git ranges
if [[ -z "$BASE_TAG" ]]; then
  RANGE="$TARGET_REF"
  DIFF_RANGE=""
else
  RANGE="${BASE_TAG}..${TARGET_REF}"
  DIFF_RANGE="${BASE_TAG}..${TARGET_REF}"
fi

# --- Collect commits by type ---------------------------------------------------

declare -a FEATURES=()
declare -a FIXES=()
declare -a PERF=()
declare -a REFACTOR=()
declare -a DOCS=()
declare -a CHORE=()
declare -a OTHER=()

while IFS= read -r line; do
  [[ -z "$line" ]] && continue

  hash="${line%% *}"
  msg="${line#* }"
  short="${hash:0:7}"

  case "$msg" in
    feat:*|feat\(*) FEATURES+=("- ${msg#feat: } (\`${short}\`)") ;;
    fix:*|fix\(*)   FIXES+=("- ${msg#fix: } (\`${short}\`)") ;;
    perf:*|perf\(*) PERF+=("- ${msg#perf: } (\`${short}\`)") ;;
    refactor:*|refactor\(*) REFACTOR+=("- ${msg#refactor: } (\`${short}\`)") ;;
    docs:*|docs\(*) DOCS+=("- ${msg#docs: } (\`${short}\`)") ;;
    chore:*|chore\(*|ci:*|ci\(*|test:*|test\(*) CHORE+=("- ${msg} (\`${short}\`)") ;;
    *) OTHER+=("- ${msg} (\`${short}\`)") ;;
  esac
done < <(git log --oneline --no-merges "$RANGE" 2>/dev/null)

# --- Stats --------------------------------------------------------------------

COMMIT_COUNT=$(git rev-list --count --no-merges "$RANGE" 2>/dev/null || echo "0")

if [[ -n "$DIFF_RANGE" ]]; then
  STAT_LINE=$(git diff --stat "$DIFF_RANGE" | tail -1)
else
  # First release — diff against empty tree
  EMPTY_TREE=$(git hash-object -t tree /dev/null)
  STAT_LINE=$(git diff --stat "$EMPTY_TREE" "$TARGET_REF" | tail -1)
fi
FILES_CHANGED=$(echo "$STAT_LINE" | grep -oE '[0-9]+ file' | grep -oE '[0-9]+' || echo "0")
INSERTIONS=$(echo "$STAT_LINE" | grep -oE '[0-9]+ insertion' | grep -oE '[0-9]+' || echo "0")
DELETIONS=$(echo "$STAT_LINE" | grep -oE '[0-9]+ deletion' | grep -oE '[0-9]+' || echo "0")

# --- Render Markdown -----------------------------------------------------------

has_section() { [[ ${#1} -gt 0 ]]; }

# Header
cat <<HEADER
> **Aura v${VERSION}** — macOS desktop companion with full computer control.

HEADER

# Features
if [[ ${#FEATURES[@]} -gt 0 ]]; then
  echo "## New Features"
  echo ""
  printf '%s\n' "${FEATURES[@]}"
  echo ""
fi

# Fixes
if [[ ${#FIXES[@]} -gt 0 ]]; then
  echo "## Bug Fixes"
  echo ""
  printf '%s\n' "${FIXES[@]}"
  echo ""
fi

# Performance
if [[ ${#PERF[@]} -gt 0 ]]; then
  echo "## Performance"
  echo ""
  printf '%s\n' "${PERF[@]}"
  echo ""
fi

# Refactoring
if [[ ${#REFACTOR[@]} -gt 0 ]]; then
  echo "## Refactoring"
  echo ""
  printf '%s\n' "${REFACTOR[@]}"
  echo ""
fi

# Docs
if [[ ${#DOCS[@]} -gt 0 ]]; then
  echo "## Documentation"
  echo ""
  printf '%s\n' "${DOCS[@]}"
  echo ""
fi

# Maintenance
if [[ ${#CHORE[@]} -gt 0 ]]; then
  echo "## Maintenance"
  echo ""
  printf '%s\n' "${CHORE[@]}"
  echo ""
fi

# Other
if [[ ${#OTHER[@]} -gt 0 ]]; then
  echo "## Other Changes"
  echo ""
  printf '%s\n' "${OTHER[@]}"
  echo ""
fi

# Stats
cat <<STATS
---

<details>
<summary><b>Release Stats</b></summary>

| Metric | Value |
|--------|-------|
| Commits | ${COMMIT_COUNT} |
| Files changed | ${FILES_CHANGED} |
| Insertions | +${INSERTIONS} |
| Deletions | -${DELETIONS} |

</details>

STATS

# Install
cat <<INSTALL
## Install

Download **Aura-${VERSION}.dmg** below, open it, and drag Aura to Applications.

Requires macOS 14+ and a [Gemini API key](https://aistudio.google.com/apikey).
INSTALL
