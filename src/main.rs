//! MP3 广告检测与裁剪工具 - CLI 入口

#[cfg(feature = "cli")]
fn main() -> anyhow::Result<()> {
    mp3cutad_rs::cli::run()
}

#[cfg(not(feature = "cli"))]
fn main() {
    println!("This is a library. Use mp3cutad-cli for command line interface.");
}
