//! GUI 应用入口与主循环

use eframe::egui;

use super::state::SharedState;
use super::{file_list, fonts, match_select, menu, results, settings, train};

/// 主应用
struct Mp3CutAdApp {
    state: SharedState,
}

impl Mp3CutAdApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        Self {
            state: Default::default(),
        }
    }
}

impl eframe::App for Mp3CutAdApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        menu::menu_bar(ctx, &self.state);
        menu::status_bar(ctx, &self.state);
        file_list::show(ctx, &self.state);
        results::show(ctx, &self.state);
        settings::show(ctx, &self.state);
        match_select::show(ctx, &self.state);
        train::show(ctx, &self.state);

        // 处理中或播放中时持续重绘
        let needs_repaint = {
            let s = self.state.lock().unwrap();
            s.is_processing || s.playback.playing_file.is_some()
        };
        if needs_repaint {
            ctx.request_repaint();
        }
    }
}

/// 启动 GUI
pub fn run() -> eframe::Result<()> {
    env_logger::init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 600.0])
            .with_min_inner_size([800.0, 400.0])
            .with_title("MP3CutAd-RS - MP3 广告检测与裁剪"),
        // 强制使用 OpenGL/Glow 渲染后端：
        // 默认 wgpu 优先走 Vulkan，在缺少 Vulkan 驱动的环境（如部分 ARM/虚拟显示）
        // 会初始化卡死；Glow 直接走 OpenGL，兼容性更好。
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "MP3CutAd-RS",
        options,
        Box::new(|cc| {
            fonts::setup_chinese_font(&cc.egui_ctx);
            Ok(Box::new(Mp3CutAdApp::new(cc)))
        }),
    )
}
