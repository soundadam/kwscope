//! egui 自带字体没有中文字形：启动时从各平台常见位置加载一款系统 CJK 字体，
//! 作为比例与等宽字体的回退，不随程序打包。

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

// (路径, ttc 内序号)。Noto Sans CJK 的 ttc 依次是 JP、KR、SC、TC、HK，取 SC。
const CANDIDATES: &[(&str, u32)] = &[
    ("/System/Library/Fonts/Hiragino Sans GB.ttc", 0),
    ("/System/Library/Fonts/STHeiti Light.ttc", 0),
    ("C:\\Windows\\Fonts\\msyh.ttc", 0),
    ("C:\\Windows\\Fonts\\simhei.ttf", 0),
    ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 2),
    ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 2),
    (
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        2,
    ),
    ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
];

/// 找到并装上返回 `true`；都找不到时界面中文显示为方块。
pub fn install(ctx: &egui::Context) -> bool {
    let Some((bytes, index)) = CANDIDATES
        .iter()
        .find_map(|&(path, index)| std::fs::read(path).ok().map(|b| (b, index)))
    else {
        return false;
    };
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert("cjk".into(), data);
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts.families.entry(family).or_default().push("cjk".into());
    }
    ctx.set_fonts(fonts);
    true
}
