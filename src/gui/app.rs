//! GUI 应用入口与主循环

use eframe::egui;

use super::state::SharedState;
use super::{fonts, panels, status_bar, sysmon, toolbar};

/// 主应用
struct Mp3CutAdApp {
    state: SharedState,
    monitor: sysmon::SysMonitor,
}

impl Mp3CutAdApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        Self {
            state: Default::default(),
            monitor: sysmon::SysMonitor::new(),
        }
    }
}

impl eframe::App for Mp3CutAdApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.monitor.sample();
        toolbar::menu_bar(ctx, &self.state);
        status_bar::status_bar(ctx, &self.state, self.monitor.snapshot());
        panels::file_list::show(ctx, &self.state);
        panels::results::show(ctx, &self.state);
        panels::settings::show(ctx, &self.state);
        panels::match_select::show(ctx, &self.state);
        panels::train::show(ctx, &self.state);

        // 处理中或播放中时持续重绘
        let needs_repaint = {
            let s = self.state.lock().unwrap();
            s.is_processing || s.playback.playing_file.is_some()
        };
        if needs_repaint {
            ctx.request_repaint();
        } else {
            // 状态栏的 CPU / GPU 占用率每秒刷新一次
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }
}

/// 窗口/任务栏图标（64×64 RGBA，由 assets/icon.png 生成）
const WINDOW_ICON: &[u8] = include_bytes!("../../assets/icon_64.rgba");

/// 启动 GUI
pub fn run() -> eframe::Result<()> {
    env_logger::init();

    let icon = egui::IconData {
        rgba: WINDOW_ICON.to_vec(),
        width: 64,
        height: 64,
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 600.0])
            .with_min_inner_size([800.0, 400.0])
            .with_icon(std::sync::Arc::new(icon))
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
