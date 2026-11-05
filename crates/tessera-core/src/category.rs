//! File categories (the `cat` byte of the layout records), by extension.
//!
//! | value | category | examples                                         |
//! |------:|----------|--------------------------------------------------|
//! | 0     | other    | anything not listed below                        |
//! | 1     | video    | mp4 mkv avi mov wmv webm ts m2ts vob             |
//! | 2     | image    | jpg png gif heic webp raw/cr2/nef psd svg        |
//! | 3     | audio    | mp3 flac wav aac ogg opus m4a wma                |
//! | 4     | archive  | zip rar 7z tar gz bz2 xz zst cab                 |
//! | 5     | vdisk    | iso img vhd vhdx vmdk vdi qcow2 wim esd          |
//! | 6     | app      | exe dll msi msix appx apk jar                    |
//! | 7     | game     | pak uasset vpk bsa ba2 bik unity3d               |
//! | 8     | doc      | pdf docx xlsx pptx txt md epub csv pst ost       |
//! | 9     | dev      | source code, build outputs (obj pdb lib), json   |
//! | 10    | cache    | tmp temp cache log dmp bak crdownload part       |
//! | 11    | system   | sys drv mui cat evtx etl fonts, NTFS metafiles   |
//!
//! Folders have no category ([`NONE`]). Matching is case-insensitive on the
//! text after the last `.`; it allocates nothing and costs one binary search
//! over a table that is sorted once per process.

use std::sync::OnceLock;

use crate::{NodeFlags, NodeId, Tree};

pub const OTHER: u8 = 0;
pub const VIDEO: u8 = 1;
pub const IMAGE: u8 = 2;
pub const AUDIO: u8 = 3;
pub const ARCHIVE: u8 = 4;
pub const VDISK: u8 = 5;
pub const APP: u8 = 6;
pub const GAME: u8 = 7;
pub const DOC: u8 = 8;
pub const DEV: u8 = 9;
pub const CACHE: u8 = 10;
pub const SYSTEM: u8 = 11;
/// No category (folders, pseudo tiles).
pub const NONE: u8 = 255;
/// Number of categories (`0..COUNT`).
pub const COUNT: usize = 12;

/// English names, indexed by category.
pub const NAMES: [&str; COUNT] = [
    "other", "video", "image", "audio", "archive", "vdisk", "app", "game", "doc", "dev", "cache",
    "system",
];

const LISTS: [(u8, &str); COUNT - 1] = [
    (
        VIDEO,
        "3g2 3gp asf avi divx f4v flv m2t m2ts m2v m4v mkv mov mp4 mpe mpeg mpg mts mxf ogv qt \
         rm rmvb ts vob webm wmv",
    ),
    (
        IMAGE,
        "ai arw avif bmp cr2 cr3 dds dng eps exr gif hdr heic heif ico jfif jpe jpeg jpg jxl \
         nef orf png psb psd raf raw rw2 svg tga tif tiff webp xcf",
    ),
    (
        AUDIO,
        "aac aif aiff alac amr ape au flac m4a m4b mid midi mka mp2 mp3 mpc oga ogg opus ra \
         wav weba wma wv",
    ),
    (
        ARCHIVE,
        "7z ace arj br bz2 cab cpio deb gz lha lz lz4 lzh lzma rar rpm tar taz tbz tbz2 tgz tlz \
         txz xz z zip zipx zst",
    ),
    (
        VDISK,
        "avhd avhdx dmg esd hdd img iso nvram ova qcow qcow2 swm vdi vhd vhdx vmdk vmem vmsn \
         vmss vsv wim",
    ),
    (
        APP,
        "apk appx appxbundle cpl dll exe jar msi msix msixbundle msp ocx scr xap",
    ),
    (
        GAME,
        "assets ba2 big bik bk2 bsa esm esp forge gcf ncf nsp pak pck ress rpf sav ubulk \
         uasset ucas umap unity3d upk utoc vpk wad xci",
    ),
    (
        DOC,
        "azw azw3 chm csv djvu doc docm docx dot dotx eml epub key md mobi msg numbers odg odp \
         ods odt one onepkg ost pages pdf pot potx pps ppsx ppt pptx pst pub rtf tex tsv txt \
         vsd vsdx xls xlsb xlsm xlsx xps",
    ),
    (
        DEV,
        "a asm c cc cjs class cmake cpp crate cs csproj css cxx dart egg el erl ex exs fs gem \
         go gradle groovy h hh hpp hs htm html idb ilk ipch java jl js json jsx kt kts less lib \
         lock lua m map mjs ml mm nupkg o obj pch pdb php pl pm ps1 psm1 py pyc pyd pyi pyo r \
         rb rlib rmeta rs sass scala scss sh sln sql swift tlog toml tsx vb vbproj vcxproj vue \
         wasm whl xaml xml yaml yml zig",
    ),
    (
        CACHE,
        "bak blockmap cache crdownload dmp download hdmp log mdmp old part partial temp tmp",
    ),
    (
        SYSTEM,
        "blf cat drv edb efi etl evtx fnt fon manifest mui mum otf regtrans-ms sdb sys ttc ttf",
    ),
];

/// Longest extension in the table.
const MAX_EXT: usize = 12;

fn table() -> &'static [(&'static str, u8)] {
    static TABLE: OnceLock<Vec<(&'static str, u8)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t: Vec<(&'static str, u8)> = LISTS
            .iter()
            .flat_map(|&(cat, list)| list.split_ascii_whitespace().map(move |e| (e, cat)))
            .collect();
        t.sort_unstable();
        t
    })
}

/// Category of a file from its name (WTF-8 or UTF-8), by extension.
pub fn of_name(name: &[u8]) -> u8 {
    let Some(dot) = name.iter().rposition(|&b| b == b'.') else {
        return OTHER;
    };
    let ext = &name[dot + 1..];
    if ext.is_empty() || ext.len() > MAX_EXT {
        return OTHER;
    }
    let mut buf = [0u8; MAX_EXT];
    for (d, &s) in buf.iter_mut().zip(ext) {
        if !s.is_ascii() {
            return OTHER;
        }
        *d = s.to_ascii_lowercase();
    }
    let key = &buf[..ext.len()];
    match table().binary_search_by(|(e, _)| e.as_bytes().cmp(key)) {
        Ok(i) => table()[i].1,
        Err(_) => OTHER,
    }
}

/// Category of a tree node: [`NONE`] for folders, [`SYSTEM`] for NTFS
/// metadata files, otherwise by extension.
pub fn of_node(tree: &Tree, id: NodeId) -> u8 {
    let n = tree.node(id);
    if n.is_dir() {
        NONE
    } else if n.flags.contains(NodeFlags::METAFILE) {
        SYSTEM
    } else {
        of_name(tree.name(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_unique_and_short() {
        let t = table();
        for w in t.windows(2) {
            assert!(w[0].0 != w[1].0, "{} listed twice", w[0].0);
        }
        for (e, _) in t {
            assert!(
                e.len() <= MAX_EXT
                    && e.bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "{e}"
            );
        }
        assert!(t.len() > 250);
    }

    #[test]
    fn lookups() {
        assert_eq!(of_name(b"Movie.MKV"), VIDEO);
        assert_eq!(of_name(b"IMG_0001.jpeg"), IMAGE);
        assert_eq!(of_name(b"song.Flac"), AUDIO);
        assert_eq!(of_name(b"backup.tar.gz"), ARCHIVE);
        assert_eq!(of_name(b"Win11.iso"), VDISK);
        assert_eq!(of_name(b"ext4.vhdx"), VDISK);
        assert_eq!(of_name(b"setup.exe"), APP);
        assert_eq!(of_name(b"pakchunk0-WindowsNoEditor.pak"), GAME);
        assert_eq!(of_name(b"report.PDF"), DOC);
        assert_eq!(of_name(b"main.rs"), DEV);
        assert_eq!(of_name(b"x.regtrans-ms"), SYSTEM);
        assert_eq!(of_name(b"pagefile.sys"), SYSTEM);
        assert_eq!(of_name(b"~DF12.TMP"), CACHE);
        assert_eq!(of_name(b"README"), OTHER);
        assert_eq!(of_name(b"trailingdot."), OTHER);
        assert_eq!(of_name(b".gitignore"), OTHER);
        assert_eq!(of_name(b"x.averyveryverylongextension"), OTHER);
        assert_eq!(of_name("photo.jpg\u{e9}".as_bytes()), OTHER);
        assert_eq!(of_name("Ünïcode name.mp3".as_bytes()), AUDIO);
        assert_eq!(NAMES[SYSTEM as usize], "system");
    }

    #[test]
    fn nodes() {
        let mut t = Tree::new(b"C:\\");
        let d = t.add_child(NodeId::ROOT, b"music.mp3", 0, 0, NodeFlags::DIR, 0);
        let m = t.add_child(NodeId::ROOT, b"$MFT", 1, 1, NodeFlags::METAFILE, 0);
        let f = t.add_child(NodeId::ROOT, b"a.mp3", 1, 1, NodeFlags::NONE, 0);
        assert_eq!(of_node(&t, d), NONE);
        assert_eq!(of_node(&t, m), SYSTEM);
        assert_eq!(of_node(&t, f), AUDIO);
    }
}
