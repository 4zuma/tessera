//! The UI language, and the user-facing text that Rust produces.
//!
//! Tessera has no language setting. At startup it reads the user's ordered
//! "Preferred languages" from Windows (Settings › Time & language › Language
//! & region) and, as Windows asks of apps, uses the first one it has a
//! translation for. When none matches, the UI is English. The WebView gets
//! the result through `platform_info`, so both sides always agree.
//!
//! `TESSERA_LANG` (e.g. `en` or `zh-CN`) overrides the list, for testing.

use std::sync::OnceLock;

/// A language Tessera has a translation for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    /// Simplified Chinese
    ZhCn,
}

impl Lang {
    /// The id of the WebView's dictionary (app/src/lib/locales).
    pub fn id(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::ZhCn => "zh-CN",
        }
    }
}

/// The language the UI uses, plus the BCP 47 tag that dates and numbers are
/// formatted with (the user's own tag when it matched, e.g. `en-GB`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub lang: Lang,
    pub locale: String,
}

static CHOICE: OnceLock<Choice> = OnceLock::new();

/// The language picked at startup (detected on first use).
pub fn choice() -> &'static Choice {
    CHOICE.get_or_init(|| {
        let forced = std::env::var("TESSERA_LANG")
            .ok()
            .filter(|s| !s.trim().is_empty());
        resolve(forced.into_iter().chain(imp::preferred_languages()))
    })
}

pub fn lang() -> Lang {
    choice().lang
}

/// The first tag in `prefs` that has a translation, else English.
pub fn resolve<S: AsRef<str>>(prefs: impl IntoIterator<Item = S>) -> Choice {
    for tag in prefs {
        let tag = tag.as_ref().trim();
        if let Some(lang) = match_tag(tag) {
            let locale = match lang {
                // keep the user's English (en-GB dates differ from en-US)
                Lang::En if is_plain_tag(tag) => tag.replace('_', "-"),
                Lang::En => "en-US".into(),
                Lang::ZhCn => "zh-CN".into(),
            };
            return Choice { lang, locale };
        }
    }
    Choice {
        lang: Lang::En,
        locale: "en-US".into(),
    }
}

/// `en-*` is English; Chinese counts when it is Simplified: `zh-Hans-*`, or
/// `zh`, `zh-CN`, `zh-SG`... without a script. Traditional Chinese (`zh-Hant`,
/// `zh-TW`, `zh-HK`, `zh-MO`) has no translation yet.
pub fn match_tag(tag: &str) -> Option<Lang> {
    let lower = tag.trim().to_ascii_lowercase().replace('_', "-");
    let mut parts = lower.split('-');
    match parts.next()? {
        "en" => Some(Lang::En),
        "zh" => {
            let rest: Vec<&str> = parts.take_while(|p| p.len() > 1).collect();
            if rest.contains(&"hans") {
                Some(Lang::ZhCn)
            } else if rest
                .iter()
                .any(|p| matches!(*p, "hant" | "tw" | "hk" | "mo"))
            {
                None
            } else {
                Some(Lang::ZhCn)
            }
        }
        _ => None,
    }
}

/// Letters, digits and dashes only, so it is safe to hand to `Intl` as is.
fn is_plain_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 35
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// One piece of user-facing text in every language Tessera speaks.
/// Adding a language adds a field, so the compiler lists what to translate.
pub struct Text {
    pub en: &'static str,
    pub zh_cn: &'static str,
}

impl Text {
    pub fn get(&self) -> &'static str {
        self.of(lang())
    }

    pub fn of(&self, lang: Lang) -> &'static str {
        match lang {
            Lang::En => self.en,
            Lang::ZhCn => self.zh_cn,
        }
    }
}

macro_rules! texts {
    ($($(#[$m:meta])* $name:ident: $en:literal, $zh:literal;)*) => {
        $($(#[$m])* pub const $name: Text = Text { en: $en, zh_cn: $zh };)*
    };
}

/// Text the backend sends to the WebView. `{}` marks where a value goes.
pub mod text {
    use super::Text;

    texts! {
        NO_SCAN: "No scan has been started yet.", "还没有开始扫描";
        NOT_FOUND: "Can't find {}", "找不到 {}";
        NO_ITEM: "This item no longer exists.", "项目不存在";
        // non-Windows dev builds only
        #[cfg_attr(windows, allow(dead_code))]
        WINDOWS_ONLY: "Only available on Windows.", "只在 Windows 上可用";
        NOT_ELEVATED: "Tessera didn't restart as administrator (perhaps “No” was chosen in the UAC prompt).",
            "没有以管理员身份重启（可能在 UAC 提示里点了“否”）";

        // volumes without a label, as Explorer names them
        LOCAL_DISK: "Local Disk", "本地磁盘";
        REMOVABLE_DISK: "Removable Disk", "可移动磁盘";

        // tiles that are not files
        FREE: "Free space", "可用空间";
        FREE_HINT: "Space on this drive that nothing uses yet", "卷上还没被使用的空间";
        UNSCANNED: "Not scanned yet", "尚未扫描";
        UNSCANNED_HINT: "Used space the scan hasn't reached yet. It shrinks as the scan goes on.",
            "已用空间里还没扫描到的部分，会随扫描缩小";
        GROUP: "Small items", "小项合并显示";
        GROUP_HINT: "Items too small to draw, merged into one tile. Zoom into the folder to see them.",
            "太小、画不下的项目合并成一块；放大上级文件夹可以看到它们";

        // hints in the details card
        HIBERFIL: "To free it, turn off hibernation: run powercfg /h off in an administrator terminal.",
            "可以在管理员终端运行 powercfg /h off 关闭休眠来释放它";
        PAGEFILE: "Change it under Advanced system settings › Performance › Virtual memory.",
            "在“高级系统设置 › 性能 › 虚拟内存”里调整";
        WINDOWS_OLD: "A backup of your previous Windows. Removing it with Disk Cleanup is safest.",
            "旧版 Windows 的备份，用系统的“磁盘清理”删除最稳妥";
        CLOUD: "Cloud file: only the part stored on this PC counts.", "云端文件：只算本机实际占用的部分";
        NO_ACCESS: "Access denied. Run Tessera as administrator to see inside.",
            "没有权限读取，以管理员身份运行可以看到里面";

        // why an item can't be deleted, or why deleting it failed
        RECYCLE_BIN: "Use “Empty Recycle Bin” instead.", "请使用“清空回收站”";
        DRIVE_ROOT: "This is the root of a drive.", "这是整个磁盘的根目录。";
        SCAN_ROOT: "This is the scanned folder itself.", "这是正在扫描的根文件夹。";
        WINDOWS: "Windows needs this folder.", "Windows 需要这个文件夹。";
        PROGRAMS: "Installed programs live in this folder.", "已安装的程序放在这个文件夹里。";
        APP_DATA: "Windows and your apps keep their data in this folder.", "Windows 和各个应用的数据存放在这里。";
        PROFILES: "This folder holds the user profiles.", "这里存放着所有用户的个人文件夹。";
        PROFILE: "This is a user profile folder.", "这是一个用户的个人文件夹。";
        // shown next to a lock: say why the folder stays, not that something may be deleted
        PERSONAL: "This is one of your personal folders. Windows and your apps use it.",
            "这是你的个人文件夹，Windows 和应用要用到它。";
        TEMP: "Windows and your apps keep temporary files in this folder.",
            "Windows 和应用把临时文件放在这个文件夹里。";
        CONTAINS: "This folder contains folders that Windows or your apps need.",
            "这个文件夹里有 Windows 或应用需要的文件夹。";
        STILL_THERE: "The item could not be deleted.", "没能删除这个项目。";
        NOT_RESTORED: "The item could not be restored.", "没能还原这个项目。";
        NAME_TAKEN: "Another item with this name is already there.", "原位置已经有同名的项目。";
        CANCELLED: "Cancelled.", "已取消。";
        SKIPPED: "Skipped.", "已跳过。";
        UNDO_GONE: "This can no longer be undone.", "这次操作已经不能撤销了。";
    }
}

#[cfg(windows)]
mod imp {
    /// The user's preferred languages, most preferred first. The WinRT list
    /// is the one in Settings; the Win32 list is the display languages (MUI),
    /// used only if WinRT is unavailable.
    pub fn preferred_languages() -> Vec<String> {
        use windows::System::UserProfile::GlobalizationPreferences;
        if let Ok(list) = GlobalizationPreferences::Languages() {
            let v: Vec<String> = list.into_iter().map(|h| h.to_string()).collect();
            if !v.is_empty() {
                return v;
            }
        }
        ui_languages()
    }

    fn ui_languages() -> Vec<String> {
        use windows::Win32::Globalization::{GetUserPreferredUILanguages, MUI_LANGUAGE_NAME};
        use windows::core::PWSTR;
        let (mut n, mut len) = (0u32, 0u32);
        if unsafe { GetUserPreferredUILanguages(MUI_LANGUAGE_NAME, &mut n, None, &mut len) }
            .is_err()
            || len == 0
        {
            return Vec::new();
        }
        let mut buf = vec![0u16; len as usize];
        if unsafe {
            GetUserPreferredUILanguages(
                MUI_LANGUAGE_NAME,
                &mut n,
                Some(PWSTR(buf.as_mut_ptr())),
                &mut len,
            )
        }
        .is_err()
        {
            return Vec::new();
        }
        // "en-US\0zh-CN\0\0"
        buf.split(|&c| c == 0)
            .filter(|s| !s.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }
}

#[cfg(not(windows))]
mod imp {
    /// Dev builds elsewhere: the usual POSIX variables (`zh_CN.UTF-8`).
    pub fn preferred_languages() -> Vec<String> {
        ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .flat_map(|v| {
                v.split(':')
                    .map(|s| s.split(['.', '@']).next().unwrap_or("").to_string())
                    .collect::<Vec<_>>()
            })
            .filter(|s| !s.is_empty() && s != "C" && s != "POSIX")
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_simplified_chinese_and_english_only() {
        for t in [
            "zh-CN",
            "zh-Hans-CN",
            "zh-Hans",
            "zh",
            "zh-SG",
            "zh_CN",
            "ZH-hans-HK",
        ] {
            assert_eq!(match_tag(t), Some(Lang::ZhCn), "{t}");
        }
        for t in ["en", "en-US", "en-GB", "EN-au"] {
            assert_eq!(match_tag(t), Some(Lang::En), "{t}");
        }
        for t in [
            "zh-TW",
            "zh-HK",
            "zh-MO",
            "zh-Hant",
            "zh-Hant-TW",
            "ja-JP",
            "fr",
            "",
            "-",
        ] {
            assert_eq!(match_tag(t), None, "{t}");
        }
    }

    #[test]
    fn the_first_language_with_a_translation_wins() {
        let c = resolve(["ja-JP", "zh-Hans-CN", "en-US"]);
        assert_eq!((c.lang, c.locale.as_str()), (Lang::ZhCn, "zh-CN"));
        let c = resolve(["en-GB", "zh-Hans-CN"]);
        assert_eq!((c.lang, c.locale.as_str()), (Lang::En, "en-GB"));
        let c = resolve(["zh-Hant-TW", "en-US"]);
        assert_eq!(c.lang, Lang::En);
    }

    #[test]
    fn falls_back_to_english() {
        assert_eq!(
            resolve(["ja-JP", "ko-KR"]),
            Choice {
                lang: Lang::En,
                locale: "en-US".into()
            }
        );
        assert_eq!(resolve(Vec::<String>::new()).lang, Lang::En);
    }

    #[test]
    fn texts_follow_the_language() {
        let t = &text::WINDOWS;
        assert_eq!(t.of(Lang::En), "Windows needs this folder.");
        assert_eq!(t.of(Lang::ZhCn), "Windows 需要这个文件夹。");
    }
}
