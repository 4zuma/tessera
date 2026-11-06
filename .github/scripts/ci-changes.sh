#!/usr/bin/env bash
# Decides which checks a push or pull request needs, from the files it changes.
# Used by the "What changed" job of ci.yml and app.yml.
#
# Writes two outputs (to $GITHUB_OUTPUT):
#   core=true|false  ci.yml: Rust core and CLI (clippy, tests, smoke scans, NTFS images)
#   app=true|false   app.yml: the UI and the Tauri app
#
# A job that is not needed is skipped with an `if:`. A skipped job still reports
# its check, as passed, so required checks on main keep working; a paths filter
# on the workflow would leave them waiting forever instead.
#
# When in doubt it says true: Run workflow, a new branch, a push it cannot diff
# or a file it does not know all run everything.
#
# Environment: EVENT (github.event_name); for pull_request BASE_SHA and HEAD_SHA,
# for push BEFORE and AFTER. Needs the history of both commits (fetch-depth: 0).

set -uo pipefail

out=${GITHUB_OUTPUT:-/dev/stdout}
summary=${GITHUB_STEP_SUMMARY:-/dev/null}

decide() {
  echo "core=$1" >>"$out"
  echo "app=$2" >>"$out"
  {
    echo "core (ci.yml): $1"
    echo "app (app.yml): $2"
    echo "$3"
  } | tee -a "$summary"
  exit 0
}

sha='^[0-9a-f]{40}$'
case ${EVENT:-} in
  pull_request)
    from=${BASE_SHA:-}
    to=${HEAD_SHA:-}
    # three dots: what the pull request changes since it left its base
    dots=...
    ;;
  push)
    from=${BEFORE:-}
    to=${AFTER:-}
    if [[ $from =~ ^0+$ ]]; then
      decide true true "A new branch: everything runs."
    fi
    dots=..
    ;;
  *)
    decide true true "${EVENT:-unknown event}: everything runs."
    ;;
esac

range="$from$dots$to"
if ! [[ $from =~ $sha && $to =~ $sha ]]; then
  decide true true "No commits to compare ($range): everything runs."
fi

if ! files=$(git -c core.quotePath=false diff --name-only --no-renames "$range" 2>&1); then
  decide true true "Could not list the changes in $range ($files): everything runs."
fi

core=false
app=false
while IFS= read -r file; do
  [[ -n $file ]] || continue
  case $file in
    # documentation, licenses, local scripts and the release workflow: nothing to check
    *.md | docs/* | LICENSE-* | scripts/* | .github/workflows/release.yml | .github/scripts/release-*) ;;
    # only the app
    app/* | .github/workflows/app.yml) app=true ;;
    # only the core and the CLI
    crates/tessera-cli/* | .github/workflows/ci.yml) core=true ;;
    # tessera-core, Cargo.toml / Cargo.lock, this script, anything new
    *)
      core=true
      app=true
      ;;
  esac
done <<<"$files"

count=$(grep -c . <<<"$files" || true)
decide "$core" "$app" "$count changed file(s) in $range."
