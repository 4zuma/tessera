/**
 * English, the base dictionary: every key exists here, and any key another language
 * leaves out falls back to this text. `{name}` marks a value filled in at runtime.
 * An entry with `one`/`other` forms is picked by the count it is used with.
 *
 * Text the Rust backend sends (lock reasons, hints, volume labels) lives in
 * app/src-tauri/src/i18n.rs; the `core.*` keys repeat it for the browser preview.
 */
export type Msg = string | { readonly one?: string; readonly other: string };

export const en = {
  /* shared words */
  free: 'Free space',
  unscanned: 'Not scanned yet',
  scanning: 'Scanning',
  folder: 'Folder',
  file: 'File',
  sizeOnDisk: 'Size on disk',
  size: 'Size',
  recycle: 'Move to Recycle Bin',
  purge: 'Delete permanently',
  emptyBin: 'Empty Recycle Bin',
  restartAdmin: 'Restart as administrator',
  cancel: 'Cancel',
  close: 'Close',
  items: { one: '{n} item', other: '{n} items' },
  files: { one: '{n} file', other: '{n} files' },
  folders: { one: '{n} folder', other: '{n} folders' },
  smallItems: { one: '{n} small item', other: '{n} small items' },
  selected: '{n} selected · {size}',
  shareParent: '{pct} of parent',
  shareDrive: '{pct} of drive',
  /** joins a few file names */
  listSep: ', ',
  /** one line of a "could not delete" list */
  failedLine: '{name}: {reason}',

  /* file types, by the core's category id */
  'cat.0': 'Other',
  'cat.1': 'Video',
  'cat.2': 'Pictures',
  'cat.3': 'Audio',
  'cat.4': 'Archives',
  'cat.5': 'Disk images',
  'cat.6': 'Programs',
  'cat.7': 'Game files',
  'cat.8': 'Documents',
  'cat.9': 'Development',
  'cat.10': 'Cache & temp',
  'cat.11': 'System',

  /* what a tile is (tooltip, details card) */
  'kind.folderMostly': 'Folder · mostly {cat}',
  'kind.group': 'Small items, grouped',
  'kind.groupShort': 'Small items',
  'kind.currentFolder': 'Current folder',

  /* why something can't be deleted (from the layout flags) */
  'lock.inUse': "Windows is using this file, so it can't be deleted",
  'lock.noAccess': 'Access denied',
  'lock.protected': "Protected, so it can't be deleted",
  'lock.bin': 'Use “Empty Recycle Bin” instead.',

  /* start */
  'app.startFailed': "Tessera couldn't start",
  'picker.title': 'Choose a drive to scan',
  'picker.sub': 'The map grows while the scan runs, so you can start exploring before it finishes.',
  'picker.space': '{free} free of {capacity} · {fs}',
  'picker.userTitle': 'Running without administrator rights',
  'picker.userBody': 'Tessera now scans folder by folder on several threads. As administrator it reads the NTFS master file table (MFT) directly, which is usually much faster.',

  /* command bar */
  'cmd.chooseDrive': 'Choose a drive',
  'cmd.drive': 'Drive',
  'cmd.driveFree': '{size} free',
  'cmd.allDrives': 'All drives…',
  'cmd.up': 'Up one level',
  'cmd.upTip': 'Up one level (Backspace)',
  'cmd.path': 'Path',
  'cmd.stop': 'Stop',
  'cmd.rescan': 'Rescan',
  'cmd.rescanTip': 'Rescan (F5)',
  'cmd.sizeMode': 'Size to show',
  'cmd.diskTip': 'The space the files take up on the disk',
  'cmd.logicalTip': 'The number of bytes in the files',
  'cmd.adminTip': 'Restart as administrator to scan faster by reading the MFT directly',
  'cmd.pane': 'Details pane',
  'cmd.showPane': 'Show details pane',
  'cmd.hidePane': 'Hide details pane',

  /* details pane */
  'pane.contains': 'Contains',
  'pane.modified': 'Modified',
  'pane.folderLocked': "This folder can't be deleted",
  'pane.fileLocked': "This file can't be deleted",
  'pane.empty': 'Point at a tile to see its details',
  /* under the greyed-out buttons while nothing is selected, one line each */
  'pane.howToSelect': 'Click to select, Ctrl or Shift + click for more',
  'pane.howToMore': 'Double-click to zoom in, right-click for more',
  'pane.byType': 'By type',
  'pane.highlight': 'Highlight only {cat}',
  'pane.afterScan': 'Shown after the scan',

  /* treemap and status bar */
  'stage.label': 'Treemap of disk usage. Arrow keys move the selection, Enter zooms in, Backspace goes up a level, Delete moves to the Recycle Bin.',
  'stage.progress': 'Scan progress',
  'stage.rects': '{n} rects',
  'stage.idle': 'Idle · 0 fps',
  'stage.draw': 'Draw {ms} ms',
  'status.used': 'Used {used} of {capacity}',
  'status.free': '{free} free',
  'status.scaleTip': 'How much one pixel stands for in this view',
  'scan.running': 'Scanning · {pct}% · {files} found',
  'scan.done': 'Scan complete · {files}, {folders} · took {time}',

  /* context menu */
  'menu.freeNote': 'The empty part of this drive. Start with the biggest tiles.',
  'menu.zoomGroup': 'Zoom in to see these items',
  'menu.zoomFolder': 'Zoom into this folder',
  'menu.reveal': 'Show in File Explorer',
  'menu.copyPath': 'Copy path',
  'menu.copyPaths': 'Copy {n} paths',
  'menu.emptyBin': 'Empty Recycle Bin…',
  'menu.purge': 'Delete permanently…',

  /* confirmations */
  'confirm.purgeTitle': 'Delete permanently?',
  'confirm.selection': 'Selected item',
  'confirm.names': '{names}. ',
  'confirm.namesMore': '{names} and more. ',
  'confirm.purgeBody': "This skips the Recycle Bin and can't be undone. It frees {size} right away.",
  'confirm.binTitle': 'Empty the Recycle Bin?',
  'confirm.binLead': 'Recycle Bin on {drive}',
  'confirm.binBody': "Everything in the Recycle Bin will be deleted permanently and can't be recovered.",

  /* notifications */
  'toast.diskMode': 'Showing size on disk: the space the files actually take up, which is what deleting them frees.',
  'toast.logicalMode': 'Showing size: the number of bytes in the files. OneDrive online-only files and compressed system files look bigger.',
  'toast.undo': 'Undo',
  'toast.recycled': 'Moved to the Recycle Bin: {what} ({size})',
  'toast.recycledBody': 'The Recycle Bin is on the same drive, so the space is freed only when you empty it.',
  'toast.notDeleted': "Some items couldn't be deleted",
  'toast.undone': 'Undone. The files are back where they were.',
  'toast.purged': 'Deleted permanently, {size} freed',
  'toast.freeNow': 'Free space is now {size}.',
  'toast.binEmptied': 'Recycle Bin emptied, {size} freed',
  'toast.binPartly': "The Recycle Bin couldn't be emptied completely",
  'toast.binLeft': 'Freed {size}. These items are still in the Recycle Bin:\n{lines}',
  'toast.more': '{n} more',
  'toast.copiedOne': 'Path copied',
  'toast.copiedMany': '{n} paths copied',
  'toast.path': 'Path',
  'err.layoutData': "Couldn't read the layout data",
  'err.layout': "Couldn't get the layout",
  'err.scanStart': "Couldn't start the scan",

  /* browser preview only */
  'preview.chip': 'Preview · sample data',
  'preview.chipTip': 'This is a browser preview with made-up disk data. The Tessera app scans your real drives.',
  'preview.download': 'Simulate download',
  'preview.downloadTip': 'Simulate a growing download to see how the map moves with live updates',
  'preview.perf': 'Performance',
  'preview.perfTip': 'Show frame rate and rect count (Ctrl+Shift+F)',
  'preview.reveal': 'The preview has no real files',
  'preview.revealBody': 'With the scan core, this runs explorer.exe /select,"{path}"',
  'preview.admin': "The preview doesn't restart",
  'preview.adminBody': 'In the Tessera app, this button restarts Tessera as administrator so it can scan by reading the MFT, which is much faster.',
  'preview.live': 'Simulated live update',
  'preview.liveBody': 'A file in Downloads is growing. A change arrives every 500 ms and the map animates between them instead of redrawing the whole screen.',

  /* units and dates */
  'fmt.bytes': { one: '{n} byte', other: '{n} bytes' },
  'fmt.sec': '{s} s',
  'fmt.minSec': '{m} min {s} s',
  'fmt.today': 'Today {time}',
  'fmt.daysAgo': { one: '{date} ({n} day ago)', other: '{date} ({n} days ago)' },
  'fmt.monthsAgo': { one: '{date} ({n} month ago)', other: '{date} ({n} months ago)' },
  'fmt.yearsAgo': { one: '{date} ({n} year ago)', other: '{date} ({n} years ago)' },

  /* the mock backend's own text (browser preview); the app's comes from Rust */
  'core.localDisk': 'Local Disk',
  'core.freeHint': 'Space on this drive that nothing uses yet',
  'core.unscannedHint': "Used space the scan hasn't reached yet. It shrinks as the scan goes on.",
  'core.group': 'Small items',
  'core.groupHint': 'Items too small to draw, merged into one tile. Zoom into the folder to see them.',
  'core.hiberfil': 'To free it, turn off hibernation: run powercfg /h off in an administrator terminal.',
  'core.pagefile': 'Change it under Advanced system settings › Performance › Virtual memory.',
  'core.windowsOld': 'A backup of your previous Windows. Removing it with Disk Cleanup is safest.',
  'core.personal': 'This is one of your personal folders. Windows and your apps use it.',
  'core.temp': 'Windows and your apps keep temporary files in this folder.',
  'core.noItem': 'This item no longer exists.',
  'mock.dataLabel': 'Data',
  'mock.systemFolder': 'System folder, protected',
  'mock.cloud': 'OneDrive online-only file: large, but takes almost no disk space',
  'mock.svi': '(no access; size estimated from the volume)',
} as const satisfies Record<string, Msg>;

export type Key = keyof typeof en;
export type Dict = { readonly [K in Key]: Msg };
