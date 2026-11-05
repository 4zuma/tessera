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
  '压缩包 / 镜像',
  '虚拟磁盘',
  '程序',
  '游戏资源',
  '文档',
  '开发',
  '缓存 / 临时',
  '系统',
];

const groups: [number, string][] = [
  [Cat.Video, 'mp4 mkv mov webm avi wmv flv m4v ts m2ts mpg mpeg 3gp'],
  [Cat.Image, 'png jpg jpeg heic heif arw cr2 cr3 nef dng raw psd webp gif bmp tif tiff svg ico avif jxl'],
  [Cat.Audio, 'flac mp3 wav m4a aac ogg opus wma ape'],
  [Cat.Archive, 'zip 7z rar iso cab esd tar gz tgz bz2 xz zst wim img crdownload part'],
  [Cat.VDisk, 'vhdx vhd vmdk vdi qcow2 avhdx'],
  [Cat.App, 'exe dll sys msi msp mui ttf otf ttc node pyd so ocx cpl drv efi appx msix'],
  [Cat.Game, 'archive vpk pak bundle assets bk2 bnk upk uasset ucas utoc forge sav'],
  [Cat.Doc, 'pdf doc docx xls xlsx ppt pptx txt md rtf odt ods odp epub csv one'],
  [Cat.Dev, 'rs js ts jsx tsx c h cpp hpp cs java kt go py rb php rlib pdb obj o a lib exp ilk json jar whl crate class pch ipch idb tlog'],
  [Cat.Cache, 'tmp temp cache dmp log old bak swp'],
  [Cat.System, 'etl dat manifest mum cat db evtx regtrans-ms blf edb sdb'],
];

export const EXT_CAT: ReadonlyMap<string, number> = new Map(
  groups.flatMap(([cat, list]) => list.split(' ').map(ext => [ext, cat] as [string, number])),
);

export function categoryOf(name: string): number {
  const dot = name.lastIndexOf('.');
  if (dot <= 0 || dot === name.length - 1) return Cat.Other;
  return EXT_CAT.get(name.slice(dot + 1).toLowerCase()) ?? Cat.Other;
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
