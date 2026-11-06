#!/usr/bin/env bash
# Publishes the files the release workflow built (out/*.exe) as decided by
# release-plan.sh. Run by .github/workflows/release.yml from the repo root.
#
#   MODE=new  create the release TAG on GITHUB_SHA with the files and their
#             SHA256SUMS.txt
#   MODE=add  upload the files the release TAG does not have yet and add their
#             checksums to its SHA256SUMS.txt
#
# The release notes are written in the description of the pull request that
# merged the release's commit (normally the version bump), between two lines
#   <!-- release-notes -->
#   <!-- /release-notes -->
# A new release without them gets notes written by GitHub. MODE=add replaces
# the notes only when that pull request has them (they may list the new files).
#
# Both check the release again first, since another run may have published
# it while this one was building. gh needs GH_TOKEN; uses GITHUB_REPOSITORY.
set -euo pipefail

: "${MODE:?}" "${TAG:?}"
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
notes=$(mktemp)

note() {
  echo "$1"
  echo "$1" >>"$summary"
}

# Writes the release notes from the pull request that merged <commit> to
# $notes. Fails when that pull request has none; a failed lookup stops the run.
pr_notes() { # pr_notes <commit>
  local start='<!-- release-notes -->' end='<!-- /release-notes -->' body text
  # an explicit exit: set -e does not apply inside "if pr_notes ..."
  body=$(gh api "repos/$GITHUB_REPOSITORY/commits/$1/pulls" --jq "
    [.[] | select(.merged_at != null) | (.body // \"\") | select(contains(\"$start\"))][0] // \"\"") || {
    echo "::error::could not look up the pull request of $1"
    exit 1
  }
  text=$(tr -d '\r' <<<"$body" | awk -v start="$start" -v end="$end" '
    { line = $0; gsub(/^[ \t]+|[ \t]+$/, "", line) }
    line == end { exit }
    on { print }
    line == start { on = 1 }' | sed '/[^[:space:]]/,$!d')
  [[ -n $text ]] || return 1
  printf '%s\n' "$text" >"$notes"
}

shopt -s nullglob
files=(out/*.exe)
((${#files[@]} > 0)) || {
  echo "::error::no files were built"
  exit 1
}

case $MODE in
  new)
    if url=$(gh release view "$TAG" --json url --jq .url 2>/dev/null); then
      echo "::error::$TAG was already released by another run: $url"
      exit 1
    fi
    (cd out && sha256sum -- *.exe >SHA256SUMS.txt && cat SHA256SUMS.txt)
    args=("$TAG" "${files[@]}" out/SHA256SUMS.txt --title "Tessera $TAG" --target "$GITHUB_SHA")
    if pr_notes "$GITHUB_SHA"; then
      args+=(--notes-file "$notes")
    else
      note "The pull request that merged $GITHUB_SHA has no release notes, so GitHub writes them."
      args+=(--generate-notes)
    fi
    # a version like 1.1.0-beta.1 is a pre-release, so it never becomes "Latest"
    if [[ $TAG == *-* ]]; then args+=(--prerelease); fi
    gh release create "${args[@]}"
    note "Published $TAG with $(printf '%s ' "${files[@]##*/}")"
    ;;
  add)
    have=$(gh release view "$TAG" --json assets --jq '.assets[].name | ascii_downcase')
    new=()
    for f in "${files[@]}"; do
      name=${f##*/}
      grep -qxF "${name,,}" <<<"$have" || new+=("$f")
    done
    if ((${#new[@]} == 0)); then
      note "$TAG already has every file (another run added them), so nothing was uploaded."
      exit 0
    fi
    # look the notes up before changing anything, so a failed lookup changes nothing
    has_notes=0
    tagged=$(gh api "repos/$GITHUB_REPOSITORY/commits/$TAG" --jq .sha)
    if pr_notes "$tagged"; then has_notes=1; fi
    mkdir -p old
    sums=""
    if grep -qxF sha256sums.txt <<<"$have"; then
      gh release download "$TAG" --pattern SHA256SUMS.txt --dir old --clobber
      sums=$(tr -d '\r' <old/SHA256SUMS.txt)
    fi
    {
      [[ -n $sums ]] && printf '%s\n' "$sums"
      (cd out && sha256sum -- "${new[@]##*/}")
    } >out/SHA256SUMS.txt
    cat out/SHA256SUMS.txt
    # the files first, so the checksum list never names a file that isn't there
    gh release upload "$TAG" "${new[@]}"
    gh release upload "$TAG" out/SHA256SUMS.txt --clobber
    if ((has_notes)); then
      gh release edit "$TAG" --notes-file "$notes" >/dev/null
      note "Updated the notes of $TAG from its pull request."
    fi
    note "Added $(printf '%s ' "${new[@]##*/}")to $TAG."
    ;;
  *)
    echo "::error::unexpected mode $MODE"
    exit 1
    ;;
esac
