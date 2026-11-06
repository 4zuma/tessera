#!/usr/bin/env bash
# Decides what the release workflow should build and publish.
# Run by .github/workflows/release.yml; writes to $GITHUB_OUTPUT
#   mode=new|add|none  publish a new release, add missing files to an existing
#                      one, or do nothing
#   tag=v<version>     the release
#   ref=<git ref>      what to build: this commit for a new release, the tag for
#                      files added to an existing one (so they match its code)
#   arches=[...]       JSON list of the architectures to build
# and one line about the decision to the job summary.
#
#   push to main      new release when the version in tauri.conf.json differs
#                     from the one before the push and the tag v<version> does
#                     not exist yet. Otherwise, if the release of the current
#                     version lacks an architecture (one was added to the
#                     workflow), build that from the tag and add it; else nothing.
#   push of tag vX    release vX; it must match tauri.conf.json at that tag
#   Run workflow      release the version typed in; it must match tauri.conf.json.
#                     For a version already released, add the missing files.
#
# A new release is refused when the versions in the repo differ
# (node scripts/bump-version.mjs --check).
#
# Uses GITHUB_EVENT_NAME, GITHUB_REF_TYPE, GITHUB_REF_NAME, GITHUB_REPOSITORY,
# GITHUB_SHA, GITHUB_OUTPUT, GITHUB_STEP_SUMMARY, plus INPUT_VERSION (Run
# workflow) and BEFORE (the commit main pointed at before the push). gh needs
# GH_TOKEN.
set -euo pipefail

# Every release has these files for each architecture, plus SHA256SUMS.txt.
ARCHES=(x64 arm64)
assets() { # assets <tag> <arch>
  echo "tessera-$1-windows-$2.exe"
  echo "tessera-cli-$1-windows-$2.exe"
}

conf=app/src-tauri/tauri.conf.json
summary=${GITHUB_STEP_SUMMARY:-/dev/null}

note() {
  echo "$1"
  echo "$1" >>"$summary"
}

finish() { # finish <mode> <tag> <ref> <arches json> <message>
  note "$5"
  {
    echo "mode=$1"
    echo "tag=$2"
    echo "ref=$3"
    echo "arches=$4"
  } >>"$GITHUB_OUTPUT"
  exit 0
}

nothing() { finish none "$1" "" "[]" "$2"; }

refuse() {
  echo "::error::$1"
  echo "Not released: $1" >>"$summary"
  exit 1
}

json_list() { # json_list a b c -> ["a","b","c"]
  local out="" a
  for a in "$@"; do out+="${out:+,}\"$a\""; done
  echo "[$out]"
}

version=$(jq -r '.version // empty' "$conf")
[[ -n $version ]] || refuse "$conf has no version"
tag="v$version"

# 0 = the tag exists, 1 = it does not; anything else stops the run, so a
# failed lookup never leads to a release.
tag_exists() {
  local rc=0
  git ls-remote --exit-code --tags origin "refs/tags/$1" >/dev/null || rc=$?
  case $rc in
    0) return 0 ;;
    2) return 1 ;;
    *) refuse "could not list the tags (git ls-remote exited with $rc)" ;;
  esac
}

# 0 = the release exists (its URL in release_url, its file names, lowercased,
# in release_assets), 1 = it does not; a failed lookup stops the run.
release_url=""
release_assets=""
release_exists() {
  local out rc=0
  out=$(gh api "repos/$GITHUB_REPOSITORY/releases/tags/$1" \
    --jq '.html_url, (.assets[].name | ascii_downcase)' 2>&1) || rc=$?
  if ((rc == 0)); then
    release_url=$(head -n 1 <<<"$out")
    release_assets=$(tail -n +2 <<<"$out")
    return 0
  fi
  [[ $out == *"HTTP 404"* ]] && return 1
  refuse "could not look up the release $1: $out"
}

# The architectures the release (looked up by release_exists) has no files for.
missing=()
find_missing() { # find_missing <tag>
  local arch name
  missing=()
  for arch in "${ARCHES[@]}"; do
    while read -r name; do
      if ! grep -qxF "$name" <<<"$release_assets"; then
        missing+=("$arch")
        break
      fi
    done < <(assets "$1" "$arch")
  done
}

# Add the missing architectures to the existing release $1, built from its tag.
add_missing() { # add_missing <tag> <message when nothing is missing>
  find_missing "$1"
  if ((${#missing[@]} == 0)); then
    nothing "$1" "$2"
  fi
  finish add "$1" "refs/tags/$1" "$(json_list "${missing[@]}")" \
    "$1 is already released; adding the ${missing[*]} build, made from the $1 tag."
}

new_release() {
  if [[ -f scripts/bump-version.mjs ]] && ! node scripts/bump-version.mjs --check; then
    refuse "the version is not the same everywhere; run node scripts/bump-version.mjs $version"
  fi
  finish new "$tag" "$GITHUB_SHA" "$(json_list "${ARCHES[@]}")" \
    "Releasing $tag from $(git rev-parse --short HEAD)${before:+ (version $before -> $version)}."
}

case ${GITHUB_EVENT_NAME:-} in
  workflow_dispatch)
    want="v${INPUT_VERSION#v}"
    if release_exists "$want"; then
      add_missing "$want" "$want is already released with every file: $release_url"
    fi
    [[ $want == "$tag" ]] ||
      refuse "$want does not match the app version $version in $conf on $GITHUB_REF_NAME; merge the version bump first"
    new_release
    ;;
  push)
    if [[ ${GITHUB_REF_TYPE:-} == tag ]]; then
      if release_exists "$GITHUB_REF_NAME"; then
        add_missing "$GITHUB_REF_NAME" "$GITHUB_REF_NAME is already released with every file."
      fi
      [[ $GITHUB_REF_NAME == "$tag" ]] ||
        refuse "tag $GITHUB_REF_NAME does not match the app version $version in $conf"
      new_release
    fi

    before=""
    if [[ -n ${BEFORE:-} && ! $BEFORE =~ ^0+$ ]] &&
      git fetch --quiet --depth=1 --no-tags origin "$BEFORE" 2>/dev/null; then
      before=$(git show "FETCH_HEAD:$conf" 2>/dev/null | jq -r '.version // empty' 2>/dev/null || true)
    fi
    if [[ $before != "$version" ]] && ! tag_exists "$tag"; then
      new_release
    fi
    if release_exists "$tag"; then
      add_missing "$tag" "$tag is already released with every file, so there is nothing to do."
    fi
    if [[ $before == "$version" ]]; then
      nothing "$tag" "The version is still $version and $tag was never released, so nothing is released from this push."
    fi
    nothing "$tag" "$tag already exists but has no release, so nothing is released from this push."
    ;;
  *)
    refuse "unexpected event ${GITHUB_EVENT_NAME:-(none)}"
    ;;
esac
