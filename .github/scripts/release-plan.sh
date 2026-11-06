#!/usr/bin/env bash
# Decides whether the release workflow should build and publish, and which tag.
# Run by .github/workflows/release.yml; writes release=true|false and tag=v<version>
# to $GITHUB_OUTPUT and one line about the decision to the job summary.
#
#   push to main      release when the version in tauri.conf.json differs from the
#                     one before the push and the tag v<version> does not exist yet;
#                     otherwise do nothing (a green run with the build skipped)
#   push of tag vX    release vX; it must match tauri.conf.json at that tag
#   Run workflow      release the version typed in; it must match tauri.conf.json
#
# Every release is refused if the release already exists or the versions in the
# repo differ (node scripts/bump-version.mjs --check).
#
# Uses GITHUB_EVENT_NAME, GITHUB_REF_TYPE, GITHUB_REF_NAME, GITHUB_REPOSITORY,
# GITHUB_OUTPUT, GITHUB_STEP_SUMMARY, plus INPUT_VERSION (Run workflow) and
# BEFORE (the commit main pointed at before the push). gh needs GH_TOKEN.
set -euo pipefail

conf=app/src-tauri/tauri.conf.json
summary=${GITHUB_STEP_SUMMARY:-/dev/null}

note() {
  echo "$1"
  echo "$1" >>"$summary"
}

finish() { # finish <release> <tag> <message>
  note "$3"
  {
    echo "release=$1"
    echo "tag=$2"
  } >>"$GITHUB_OUTPUT"
  exit 0
}

refuse() {
  echo "::error::$1"
  echo "Not released: $1" >>"$summary"
  exit 1
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

case ${GITHUB_EVENT_NAME:-} in
  workflow_dispatch)
    want="v${INPUT_VERSION#v}"
    [[ $want == "$tag" ]] ||
      refuse "$want does not match the app version $version in $conf on $GITHUB_REF_NAME; merge the version bump first"
    ;;
  push)
    if [[ ${GITHUB_REF_TYPE:-} == tag ]]; then
      [[ $GITHUB_REF_NAME == "$tag" ]] ||
        refuse "tag $GITHUB_REF_NAME does not match the app version $version in $conf"
    else
      before=""
      if [[ -n ${BEFORE:-} && ! $BEFORE =~ ^0+$ ]] &&
        git fetch --quiet --depth=1 --no-tags origin "$BEFORE" 2>/dev/null; then
        before=$(git show "FETCH_HEAD:$conf" 2>/dev/null | jq -r '.version // empty' 2>/dev/null || true)
      fi
      if [[ $before == "$version" ]]; then
        finish false "$tag" "The version is still $version, so there is nothing to release."
      fi
      if tag_exists "$tag"; then
        finish false "$tag" "The version changed to $version, but $tag already exists, so it is not released again."
      fi
    fi
    ;;
  *)
    refuse "unexpected event ${GITHUB_EVENT_NAME:-(none)}"
    ;;
esac

if url=$(gh release view "$tag" --json url --jq .url 2>/dev/null); then
  refuse "$tag is already released: $url"
fi

if [[ -f scripts/bump-version.mjs ]] && ! node scripts/bump-version.mjs --check; then
  refuse "the version is not the same everywhere; run node scripts/bump-version.mjs $version"
fi

finish true "$tag" "Releasing $tag from $(git rev-parse --short HEAD)${before:+ (version $before -> $version)}."
