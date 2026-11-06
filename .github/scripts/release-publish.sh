#!/usr/bin/env bash
# Publishes the files the release workflow built (out/*.exe) as decided by
# release-plan.sh. Run by .github/workflows/release.yml from the repo root.
#
#   MODE=new  create the release TAG on GITHUB_SHA with the files and their
#             SHA256SUMS.txt; notes from docs/releases/<tag>.md when it exists,
#             otherwise GitHub writes them
#   MODE=add  upload the files the release TAG does not have yet, add their
#             checksums to its SHA256SUMS.txt, and refresh its notes from
#             docs/releases/<tag>.md when it exists (it may list the new files)
#
# Both check the release again first, since another run may have published
# it while this one was building. gh needs GH_TOKEN.
set -euo pipefail

: "${MODE:?}" "${TAG:?}"
summary=${GITHUB_STEP_SUMMARY:-/dev/null}
notes="docs/releases/$TAG.md"

note() {
  echo "$1"
  echo "$1" >>"$summary"
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
    if [[ -f $notes ]]; then args+=(--notes-file "$notes"); else args+=(--generate-notes); fi
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
    if [[ -f $notes ]]; then gh release edit "$TAG" --notes-file "$notes"; fi
    note "Added $(printf '%s ' "${new[@]##*/}")to $TAG."
    ;;
  *)
    echo "::error::unexpected mode $MODE"
    exit 1
    ;;
esac
