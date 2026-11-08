//! 中文字体加载

use egui::Context;

/// 设置中文字体
pub fn setup_chinese_font(ctx: &Context) {
    let mut fonts = egui::FontDefinitions::default();

    // 内置字体文件
    let font_data = include_bytes!("../../assets/fonts/NotoSansSC-Regular.ttf");

    fonts.font_data.insert(
        "NotoSansSC".to_owned(),
        egui::FontData::from_static(font_data),
    );

    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "NotoSansSC".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "NotoSansSC".to_owned());

    ctx.set_fonts(fonts);
}
