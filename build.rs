//! Windows 资源：给可执行文件嵌入图标与版本信息

fn main() {
    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "MP3CutAd-RS");
        res.set("FileDescription", "MP3 广告检测与裁剪");
        res.set("LegalCopyright", "MIT License");
        res.set("OriginalFilename", "MP3CutAd-RS.exe");
        if let Err(err) = res.compile() {
            // 缺少 rc.exe 时不让构建失败，只失去 exe 图标
            println!("cargo:warning=嵌入 Windows 图标失败: {err}");
        }
    }
}
