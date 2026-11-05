/**
 * File categories (the `cat` byte of a layout record). The Rust core keeps the same
 * ids in tessera_core::category; keep the two tables in sync.
 */
export const Cat = {
  Other: 0,
  Video: 1,
  Image: 2,
  Audio: 3,
  Archive: 4,
  VDisk: 5,
  App: 6,
  Game: 7,
  Doc: 8,
  Dev: 9,
  Cache: 10,
  System: 11,
} as const;

export const CAT_COUNT = 12;

export const CAT_NAMES: readonly string[] = [
  '其他',
  '视频',
  '图片',
  '音频',
  '压缩包',
  '磁盘镜像',
  '程序',
  '游戏资源',
  '文档',
  '开发',
  '缓存 / 临时',
  '系统',
];

// Copied from tessera_core::category (crates/tessera-core/src/category.rs), which is
// what the real app uses. Only the mock reads this table; keep the two in sync.
const groups: [number, string][] = [
  [Cat.Video, '3g2 3gp asf avi divx f4v flv m2t m2ts m2v m4v mkv mov mp4 mpe mpeg mpg mts mxf ogv qt rm rmvb ts vob webm wmv'],
  [Cat.Image, 'ai arw avif bmp cr2 cr3 dds dng eps exr gif hdr heic heif ico jfif jpe jpeg jpg jxl nef orf png psb psd raf raw rw2 svg tga tif tiff webp xcf'],
  [Cat.Audio, 'aac aif aiff alac amr ape au flac m4a m4b mid midi mka mp2 mp3 mpc oga ogg opus ra wav weba wma wv'],
  [Cat.Archive, '7z ace arj br bz2 cab cpio deb gz lha lz lz4 lzh lzma rar rpm tar taz tbz tbz2 tgz tlz txz xz z zip zipx zst'],
  [Cat.VDisk, 'avhd avhdx dmg esd hdd img iso nvram ova qcow qcow2 swm vdi vhd vhdx vmdk vmem vmsn vmss vsv wim'],
  [Cat.App, 'apk appx appxbundle cpl dll exe jar msi msix msixbundle msp ocx scr xap'],
  [Cat.Game, 'assets ba2 big bik bk2 bnk bsa bundle esm esp forge gcf ncf nsp pak pck ress rpf sav ubulk uasset ucas umap unity3d upk utoc vpk wad xci'],
  [Cat.Doc, 'azw azw3 chm csv djvu doc docm docx dot dotx eml epub key md mobi msg numbers odg odp ods odt one onepkg ost pages pdf pot potx pps ppsx ppt pptx pst pub rtf tex tsv txt vsd vsdx xls xlsb xlsm xlsx xps'],
  [Cat.Dev, 'a asm c cc cjs class cmake cpp crate cs csproj css cxx dart egg el erl ex exs fs gem go gradle groovy h hh hpp hs htm html idb ilk ipch java jl js json jsx kt kts less lib lock lua m map mjs ml mm nupkg o obj pch pdb php pl pm ps1 psm1 py pyc pyd pyi pyo r rb rlib rmeta rs sass scala scss sh sln sql swift tlog toml tsx vb vbproj vcxproj vue wasm whl xaml xml yaml yml zig'],
  [Cat.Cache, 'bak blockmap cache crdownload dmp download hdmp log mdmp old part partial swp temp tmp'],
  [Cat.System, 'blf cat drv edb efi etl evtx fnt fon manifest mui mum otf regtrans-ms sdb sys ttc ttf'],
];

export const EXT_CAT: ReadonlyMap<string, number> = new Map(
  groups.flatMap(([cat, list]) => list.split(' ').map(ext => [ext, cat] as [string, number])),
);

/** `.ts` files smaller than this are TypeScript, not MPEG transport streams (as in the core). */
export const TS_VIDEO_MIN = 1 << 20;

/** Category of a file by extension; pass its length to apply the core's `.ts` rule. */
export function categoryOf(name: string, logical?: number): number {
  const dot = name.lastIndexOf('.');
  if (dot <= 0 || dot === name.length - 1) return Cat.Other;
  const ext = name.slice(dot + 1).toLowerCase();
  const cat = EXT_CAT.get(ext) ?? Cat.Other;
  if (cat === Cat.Video && ext === 'ts' && logical != null && logical < TS_VIDEO_MIN) return Cat.Dev;
  return cat;
}

/**
 * Base hues from the Windows 11 palette (the same family the Settings app uses for its
 * storage breakdown). The painter mixes them toward the surface per theme.
 */
export const CAT_BASE: readonly string[] = [
  '#8E8CD8', // other
  '#8764B8', // video
  '#0099BC', // image
  '#E3008C', // audio
  '#CA5010', // archive
  '#4F6BED', // vdisk
  '#038387', // app
  '#498205', // game
  '#B89500', // doc
  '#5C2E91', // dev
  '#7A7574', // cache
  '#69797E', // system
];
