//! 图例与配色

use std::collections::HashMap;

pub(crate) fn ad_colors() -> Vec<egui::Color32> {
    vec![
        egui::Color32::from_rgb(200, 80, 80),
        egui::Color32::from_rgb(80, 160, 200),
        egui::Color32::from_rgb(200, 160, 60),
        egui::Color32::from_rgb(120, 200, 120),
        egui::Color32::from_rgb(180, 100, 200),
        egui::Color32::from_rgb(200, 120, 60),
    ]
}
pub(crate) fn confidence_color(
    score: f64,
    fallback: egui::Color32,
    thresholds: crate::gui::state::Thresholds,
) -> egui::Color32 {
    if score <= 0.0 {
        fallback
    } else if score >= thresholds.score_top {
        egui::Color32::from_rgb(160, 24, 44)
    } else if score >= thresholds.score_high {
        egui::Color32::from_rgb(200, 80, 80)
    } else if score >= thresholds.score_low {
        egui::Color32::from_rgb(226, 146, 60)
    } else {
        egui::Color32::from_rgb(198, 188, 66)
    }
}

pub(crate) fn confidence_legend(ui: &mut egui::Ui, thresholds: crate::gui::state::Thresholds) {
    let top = thresholds.score_top;
    let high = thresholds.score_high;
    let low = thresholds.score_low;
    let items = [
        (
            format!("≥{:.0}%", top * 100.0),
            confidence_color(top, egui::Color32::WHITE, thresholds),
        ),
        (
            format!("{:.0}-{:.0}%", high * 100.0, top * 100.0),
            confidence_color((high + top) / 2.0, egui::Color32::WHITE, thresholds),
        ),
        (
            format!("{:.0}-{:.0}%", low * 100.0, high * 100.0),
            confidence_color((low + high) / 2.0, egui::Color32::WHITE, thresholds),
        ),
        (
            format!("<{:.0}%", low * 100.0),
            confidence_color(low * 0.5, egui::Color32::WHITE, thresholds),
        ),
    ];
    for (text, color) in items {
        ui.label(
            egui::RichText::new(format!("● {text}"))
                .small()
                .color(color),
        );
    }
    ui.label(
        egui::RichText::new("匹配度")
            .small()
            .color(egui::Color32::from_gray(150)),
    );
}

pub(crate) fn event_palette() -> Vec<egui::Color32> {
    vec![
        egui::Color32::from_rgb(100, 200, 255),  // 浅蓝
        egui::Color32::from_rgb(150, 255, 150),  // 浅绿
        egui::Color32::from_rgb(255, 180, 100),  // 橙黄
        egui::Color32::from_rgb(200, 150, 255),  // 淡紫
        egui::Color32::from_rgb(100, 255, 220),  // 青
        egui::Color32::from_rgb(255, 140, 180),  // 粉红
        egui::Color32::from_rgb(255, 230, 100),  // 亮黄
        egui::Color32::from_rgb(140, 200, 140),  // 苔绿
    ]
}

pub(crate) fn event_legend(
    ui: &mut egui::Ui,
    labels: &[String],
    label_colors: &HashMap<String, egui::Color32>,
) {
    ui.label(
        egui::RichText::new("🎯 事件")
            .small()
            .color(egui::Color32::from_gray(180)),
    );
    for label in labels {
        let color = label_colors
            .get(label)
            .copied()
            .unwrap_or(egui::Color32::GRAY);
        ui.label(
            egui::RichText::new(format!("▢ {}", label))
                .small()
                .color(color),
        );
    }
}
