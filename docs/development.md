# Developing Tessera

The [README](../README.md) is for people who use Tessera. This page is for people who work
on it: how the core works, how to build and test it, and how CI and releases run. The UI has
its own notes in [`app/README.md`](../app/README.md), and the contract between the core and
the UI is in [`ipc.md`](ipc.md).

* `crates/tessera-core` – the Rust core: scanner, file tree, live updates, deletion, treemap
  layout.
* `crates/tessera-cli` – the command-line harness, `tessera`.
* `app/` – the Tauri 2 + Svelte app.
* `scripts/` – the version bump, the app icon (`brand-icons.mjs`, see
  [`brand/README.md`](brand/README.md)), the README animation and the idle-CPU
  measurement.

## What the core does

* **Scanning without admin rights.** A multithreaded directory walk over
  `NtQueryDirectoryFile` (one 64 KiB buffer per thread, a rayon pool sized
  from the CPU count). Hard links are counted once, junctions and symlinks
  are not followed, cloud placeholders are listed without being downloaded.
* **Scanning with admin rights.** On NTFS volumes Tessera reads the master
  file table directly (skipping free regions, parsing in parallel) and
  builds the same tree in a couple of seconds.
* **Compact tree.** Every file or folder is a 20-byte node in a chunked
  arena. A file's node holds its logical size and how its size on disk
  follows from it (rounded up to the cluster size for nearly every file);
  folders, files of 4 GiB or more and files with any other size on disk
  (compressed, sparse) have a 16-byte record more. Names are stored once
  in a shared pool (identical names are de-duplicated during a scan) and
  compressed with a symbol table. During a
  scan the table is trained on the first megabyte of names; when the scan
  ends, the names are compressed again with a table trained on a sample of
  the whole tree. Together that saves about a third. Each node keeps
  its size on disk, logical size and flags; a file's last-write time is read
  from disk when its details are shown. The full tree stays in Rust; the UI
  only ever receives the rectangles it draws.
* **Live updates.** After a scan, `ReadDirectoryChangesExW` (or the NTFS
  USN journal after an MFT scan) keeps the tree current, applied in batches
  at most every 500 ms. Idle CPU use is near zero.
* **Treemap layout.** A squarified layout that returns one compact binary
  buffer per view (see `crates/tessera-core/src/layout.rs`), with rank
  hysteresis so tiles don't jump around between updates.
* **Deletion.** Move to the Recycle Bin (with undo) or delete permanently
  through the Windows shell (`IFileOperation`), with friendly reasons for
  locked or protected items.

## Command-line harness

```text
tessera scan  <PATH> [--mode auto|walk|mft] [--threads N] [--top N] [--json]
tessera bench <PATH> [--mode auto|walk|mft] [--threads N] [--runs N] [--idle SECS] [--watch]
tessera watch <PATH> [--mode auto|walk|mft] [--secs N]
```

`scan` prints totals, memory use and the largest files. `bench` scans
several times, times treemap layouts of the result, then measures idle CPU
(with `--watch`: with live updates running) and prints one JSON object.
`watch` prints each batch of live updates.

`--mode auto` (the default) reads the MFT when the process is elevated and
the path is an NTFS volume root, and walks the directories otherwise.

## Building and testing

Requires Rust 1.85 or newer (the app's Tauri crate needs 1.90).

```sh
cargo build --release -p tessera-cli
cargo test --workspace
```

The MFT and USN-journal tests need an elevated prompt; they are ignored by
default. Run them with:

```powershell
$env:TESSERA_ADMIN_TESTS = "1"
cargo test -p tessera-core --test mft --test watch -- --include-ignored
```

The MFT parser is also tested against real NTFS images built with `mkntfs`
and `ntfs-3g` on Linux (`cargo test -p tessera-core --lib image_tests`).
The scanner itself only runs on Windows.

To see where the app's memory goes, start it with `TESSERA_MEMLOG` set to
a file: every 2 seconds it appends a line with its private working set,
heaps, other private memory and the file tree's share, plus a line for each
private region of 1 MiB or more that appears or goes away when such regions
change by 4 MiB or more.

The app is built and run from `app/`; see [`app/README.md`](../app/README.md).

## The README animation

The animation at the top of the README is the browser preview with its sample data: it scans
C:, zooms into Program Files (x86) and goes back up. `scripts/readme-media.mjs` records it in
English and Chinese, light and dark, and writes `docs/readme/demo-*.webp` (animated WebP) and
`docs/readme/still-*.webp` (the still shown to visitors who ask for reduced motion). After a
change to the UI, record it again:

```sh
cd app && npm install && cd ..
npm i -g playwright && npx playwright install chromium   # once
node scripts/readme-media.mjs            # all four, or e.g. `zh dark` for one
```

Record on Windows, so the text is set in Segoe UI as in the app; there the script uses
Microsoft Edge if Playwright's Chromium is not installed. The page runs on a fake clock that the
script moves on one frame at a time, so the result does not depend on the machine's speed. If
the sample data or the layout changes, the script stops with a message when its double-click no
longer lands on Program Files (x86); update `ZOOM_AT` in the script.

## Continuous integration

Each push and pull request runs only the checks its files can affect
(`.github/scripts/ci-changes.sh`): a change to docs only (`*.md`, `docs/`)
runs none, a change to the app only skips the core's jobs, and a change to
the CLI only skips the app's. A skipped job still reports its check as
passed, so required checks keep working.

The app exe is built on `main`. For a test build of a pull request, add the
`build-exe` label to it: that starts an `app` run whose artifact
`tessera-app-windows-x64` holds the exe, and later pushes build one too.

The smoke scan walks `C:\Users` and `C:\ProgramData`, since a walk of all of
C: takes up to 35 minutes on GitHub's runners; run the `CI` workflow by hand
with *full_walk* to walk the whole drive.

## Releasing

1. Set the new version everywhere it is written down (Cargo, npm and Tauri
   each keep a copy):

   ```sh
   node scripts/bump-version.mjs 0.3.1
   ```

   `node scripts/bump-version.mjs --check` lists the versions and fails if
   they differ.
2. Write the release notes, in English, in the pull request's description,
   between a line `<!-- release-notes -->` and a line `<!-- /release-notes -->`
   (GitHub doesn't show either). That text becomes the release notes; without
   it GitHub writes them from the merged pull requests. To fix the notes of a
   published release, edit them on the Releases page.
3. Commit, open a pull request, and merge it once CI is green. Merging is
   the release: when a push to `main` changes the version, the `release`
   workflow builds the Windows binaries (x64 and Arm64), tags the merge commit
   `v0.3.1` and publishes the release. Pushes that leave the version alone, or whose tag
   already exists, release nothing.
4. To release by hand (for example after a failed run), run the `release`
   workflow from the Actions tab on `main` with version `0.3.1`. It refuses a
   version that differs from the one in the code, and takes the notes from
   the pull request that merged the commit it runs on. For a version that is
   already released it only adds missing files, built from that version's
   tag; the current release gets them by itself when the workflow gains a
   new architecture.
