//! File-type glyphs and colors for the explorer, tabs and quick-open list.
//!
//! With `--nerd-font` (or `TERMED_NERD_FONT=1`) real Nerd Font devicons are
//! used; otherwise a portable set of geometric glyphs that render in any font.

use ratatui::style::Color;

use crate::theme::hex;

pub struct Icon {
    pub glyph: &'static str,
    pub color: Color,
}

pub fn file_icon(name: &str, nerd: bool) -> Icon {
    let lower = name.to_lowercase();
    let ext = lower.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let (n, u, c): (&str, &str, &str) = match (lower.as_str(), ext) {
        ("dockerfile", _) | ("containerfile", _) => ("\u{f308}", "◆", "#2496ED"),
        ("makefile", _) | ("cmakelists.txt", _) => ("\u{e779}", "◆", "#6D8086"),
        ("cargo.toml", _) | ("cargo.lock", _) => ("\u{e7a8}", "◆", "#DEA584"),
        (".gitignore", _) | (".gitattributes", _) | (".gitmodules", _) => {
            ("\u{e702}", "◆", "#F14E32")
        }
        ("license", _) | ("license.md", _) | ("license.txt", _) => ("\u{f0219}", "◆", "#D0BF41"),
        (_, "cs") | (_, "csx") => ("\u{f031b}", "◆", "#9B4F96"),
        (_, "rs") => ("\u{e7a8}", "◆", "#DEA584"),
        (_, "py") | (_, "pyi") => ("\u{e73c}", "◆", "#3572A5"),
        (_, "js") | (_, "mjs") | (_, "cjs") => ("\u{e74e}", "◆", "#F1E05A"),
        (_, "ts") | (_, "mts") => ("\u{e628}", "◆", "#3178C6"),
        (_, "tsx") | (_, "jsx") => ("\u{e7ba}", "◆", "#61DAFB"),
        (_, "go") => ("\u{e627}", "◆", "#00ADD8"),
        (_, "java") => ("\u{e738}", "◆", "#E76F00"),
        (_, "kt") | (_, "kts") => ("\u{e634}", "◆", "#A97BFF"),
        (_, "c") | (_, "h") | (_, "hx") | (_, "hpp") | (_, "hh") | (_, "hxx") | (_, "pch") => ("\u{e61e}", "◆", "#599EFF"),
        (_, "cpp") | (_, "cc") | (_, "cxx") | (_, "hlsl") | (_, "glsl") | (_, "shader") => {
            ("\u{e61d}", "◆", "#F34B7D")
        }
        (_, "swift") => ("\u{e755}", "◆", "#F05138"),
        (_, "rb") => ("\u{e739}", "◆", "#CC342D"),
        (_, "php") => ("\u{e73d}", "◆", "#777BB4"),
        (_, "lua") => ("\u{e620}", "◆", "#51A0CF"),
        (_, "html") | (_, "htm") => ("\u{e736}", "◇", "#E44D26"),
        (_, "css") | (_, "scss") | (_, "sass") | (_, "less") => ("\u{e749}", "◇", "#42A5F5"),
        (_, "json") | (_, "jsonc") => ("\u{e60b}", "●", "#CBCB41"),
        (_, "toml") | (_, "ini") | (_, "cfg") | (_, "conf") => ("\u{e615}", "●", "#9C4221"),
        (_, "yaml") | (_, "yml") => ("\u{e6a8}", "●", "#CB171E"),
        (_, "xml") | (_, "csproj") | (_, "sln")| (_, "slnx") | (_, "props") => ("\u{f05c0}", "◇", "#E37933"),
        (_, "md") | (_, "markdown") => ("\u{e73e}", "■", "#519ABA"),
        (_, "txt") | (_, "log") => ("\u{f0219}", "■", "#89E051"),
        (_, "sh") | (_, "bash") | (_, "zsh") | (_, "fish") | (_, "ps1") => {
            ("\u{e795}", "▸", "#4EAA25")
        }
        (_, "sql") => ("\u{e706}", "●", "#DAD8D8"),
        (_, "lock") => ("\u{f023}", "■", "#888888"),
        (_, "png")
        | (_, "jpg")
        | (_, "jpeg")
        | (_, "gif")
        | (_, "svg")
        | (_, "ico")
        | (_, "webp") => ("\u{f1c5}", "▣", "#A074C4"),
        (_, "zig") => ("\u{e6a9}", "◆", "#F7A41D"),
        (_, "hs") => ("\u{e777}", "◆", "#A074C4"),
        (_, "ex") | (_, "exs") => ("\u{e62d}", "◆", "#A074C4"),
        (_, "dart") => ("\u{e798}", "◆", "#03589C"),
        (_, "vue") => ("\u{e6a0}", "◆", "#41B883"),
        _ => ("\u{f0214}", "■", "#7F8C98"),
    };
    Icon {
        glyph: if nerd { n } else { u },
        color: hex(c),
    }
}

pub fn folder_icon(open: bool, nerd: bool) -> Icon {
    let glyph = match (nerd, open) {
        (true, true) => "\u{f0770}",
        (true, false) => "\u{f024b}",
        (false, true) => "▾",
        (false, false) => "▸",
    };
    Icon {
        glyph,
        color: hex("#C09553"),
    }
}
