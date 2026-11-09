# MP3CutAd-RS

音频广告检测与裁剪工具。通过音频指纹匹配，找出多个音频文件中重复出现的片段（通常是广告），并自动裁剪掉这些片段。

> **项目来源**：本项目参考 GitHub 上的同名项目 [licstar/MP3CutAd](https://github.com/licstar/MP3CutAd)（C# 实现），在其算法思路基础上使用 **Rust 重写**，并重新组织了代码结构、补充了图形界面和多格式支持。

## 功能特性

- **多格式支持**：MP3 / WAV / FLAC / M4A / AAC / OGG / OPUS / WMA，裁剪输出保持源格式，无需先转成 MP3
- **图形界面**：文件列表、检测进度、可视化时间轴、广告段点击试听、多选与批量保存
- **命令行**：支持检测、裁剪、一键处理，适合批量任务
- **C ABI 动态库**：便于其他语言或宿主程序集成
- **并行计算**：基于 Rayon 自动利用多核 CPU
- **结果缓存**：音频特征缓存到系统临时目录，重复检测时跳过解码
- **精确检测**：FFT 频谱分析 + LSH 局部敏感哈希 + 多特征融合 + Viterbi 时序过滤

## 前置要求

- **FFmpeg**：必须安装并加入系统 `PATH`（所有解码与裁剪都通过 ffmpeg 完成）
- **Rust 1.70+**：仅编译时需要
- Windows 10/11 通常已自带 VC++ 运行库；极老系统如需运行 release 版可安装 Microsoft Visual C++ Redistributable (x64)

检查 FFmpeg 是否可用：

```bash
mp3cutad-cli check
```

## 构建

两个可执行目标都通过 Cargo feature 控制，**直接执行 `cargo build --release` 只会构建库和 DLL**，不会生成 CLI 与 GUI。

### 方式一：Windows 一键脚本

```bat
build.bat
```

脚本依次构建库、CLI、GUI，并把产物复制到 `release\` 目录：

- `release\MP3CutAd-RS.exe`（由 `mp3cutad-gui.exe` 复制而来）
- `release\mp3cutad-cli.exe`
- `release\mp3cutad_rs.dll`

### 方式二：手动构建

```bash
# 库 + C ABI 动态库
cargo build --release --lib

# CLI
cargo build --release --features cli --bin mp3cutad-cli

# GUI
cargo build --release --features gui --bin mp3cutad-gui

# 一次性构建 CLI 与 GUI
cargo build --release --features cli,gui
```

产物位于 `target/release/`：

| 文件 | 说明 |
|---|---|
| `mp3cutad-cli.exe` | 命令行程序 |
| `mp3cutad-gui.exe` | 图形界面程序 |
| `mp3cutad_rs.dll` | C ABI 动态库 |

分发时可将 `mp3cutad-gui.exe` 重命名为 `MP3CutAd-RS.exe`，或直接运行 `build.bat` 让它自动完成复制。三个文件均可单独拷出使用，但运行环境仍需安装 FFmpeg 并加入 `PATH`。

### 开发与调试

开发时使用 Debug 构建（不加 `--release`），编译更快，产物位于 `target/debug/`：

```bash
# 构建 CLI 与 GUI 的调试版本
cargo build --features cli,gui

# 直接运行（`--` 之后的参数传给程序本身）
cargo run --features cli --bin mp3cutad-cli -- check
cargo run --features cli --bin mp3cutad-cli -- detect a.mp3 b.mp3
cargo run --features gui --bin mp3cutad-gui
```

常用检查命令：

```bash
cargo fmt                                              # 格式化
cargo fmt --check                                      # 仅检查格式
cargo check --all-targets --features cli,gui           # 类型检查（含测试目标）
cargo clippy --all-targets --features cli,gui          # 静态检查
cargo test --lib                                       # 运行全部单元测试
cargo test --lib viterbi -- --nocapture                # 只跑匹配的测试并显示输出
```

GUI 通过 `env_logger` 初始化日志，需要排查问题时可打开调试日志：

```powershell
$env:RUST_LOG="debug"; cargo run --features gui --bin mp3cutad-gui
```

## 图形界面

直接运行 `mp3cutad-gui.exe`（或 `MP3CutAd-RS.exe`）：

1. **添加文件 / 添加目录**：支持上述 8 种音频格式
2. **检测广告**：状态栏显示实时进度（预处理音频 i/N、比对音频片段 i/M）
3. **查看结果**：每个文件一条时间轴，按原始时长绘制，广告段以色块标注
4. **试听**：点击色块播放该广告段，再次点击停止
5. **多选**：`Ctrl + 点击` 可多选广告段（黄色边框）
6. **保存**：可输出到目录或勾选“覆盖原文件”；覆盖时先裁剪到临时文件，成功后才替换原文件

## 命令行用法

### 检测广告

```bash
mp3cutad-cli detect file1.mp3 file2.flac -o result.txt
```

### 裁剪（使用已有检测结果）

```bash
mp3cutad-cli cut -i input_dir -o output_dir -r result.txt
```

### 一键检测并裁剪

```bash
mp3cutad-cli auto -i input_dir -o output_dir
```

### 参数说明

```
mp3cutad-cli detect [OPTIONS] <FILES>...
  <FILES>...                音频文件列表
  -o, --output <OUTPUT>     输出检测结果到文件

mp3cutad-cli cut [OPTIONS] -i <INPUT_DIR> -o <OUTPUT_DIR> -r <RESULT_FILE>
  -i, --input-dir <DIR>     输入目录
  -o, --output-dir <DIR>    输出目录
  -r, --result-file <FILE>  检测结果文件

mp3cutad-cli auto [OPTIONS] -i <INPUT_DIR> -o <OUTPUT_DIR>
  -i, --input-dir <DIR>     输入目录
  -o, --output-dir <DIR>    输出目录
```

### 退出与跳过规则

- 某个文件全部内容都被判定为广告时，该文件会报错跳过，不影响其余文件
- 检测结果只保留至少被 2 个文件匹配到的区间（`出现次数 >= 2`）

## 算法原理

1. **解码**：调用 FFmpeg 将源音频转为单声道 22050Hz WAV
2. **频谱分析**：分帧做 FFT，得到频谱图
3. **特征提取**：FFT 频谱 + MFCC + Chroma 多特征
4. **指纹哈希**：LSH（局部敏感哈希）生成音频指纹，支持多探针查询
5. **相似度匹配**：文件两两比较，计算指纹余弦相似度（x86_64 下使用 AVX2）
6. **时序过滤**：Viterbi 算法保证匹配区间在时间上连续一致
7. **区间合并**：合并连续匹配片段，得到广告区间
8. **裁剪拼接**：移除广告区间，拼接剩余片段并保持原格式

## 输出格式与缓存

**输出格式策略**（裁剪拼接时）：

| 格式 | 策略 |
|---|---|
| MP3 / WAV | 流拷贝（`-c copy`），无重编码、无质量损失 |
| FLAC | 无损重编码，保证时长元数据正确 |
| OGG / OPUS / M4A / AAC / WMA | 重编码一次（流拷贝拼接易导致时间戳与时长错误） |

**缓存**：检测中间结果（哈希、特征）缓存在系统临时目录下的 `mp3cutad_cache`，以完整路径 + 文件修改时间校验，文件变更或缓存损坏会自动重算。删除该目录不影响使用，只是下次需要重新计算。

## 项目结构

```
src/
├── lib.rs            # 库入口与公共导出
├── main.rs           # CLI 入口（薄壳）
├── gui_main.rs       # GUI 入口（薄壳）
├── core/             # 纯算法层（不执行外部程序）
│   ├── fft.rs        # FFT 频谱分析
│   ├── lsh.rs        # 局部敏感哈希与相似度
│   ├── features.rs   # MFCC / Chroma 多特征
│   ├── viterbi.rs    # 时序一致性过滤
│   ├── detect.rs     # 广告检测流程（含进度回调）
│   └── range.rs      # 区间数据结构
├── audio/            # 音频 I/O
│   ├── ffmpeg.rs     # FFmpeg 封装（解码 / 裁剪 / 拼接）
│   └── cache.rs      # 检测中间结果缓存
├── pipeline.rs       # 检测 → 裁剪的流程编排
├── result_io.rs      # 检测结果文件读写
├── cli/              # 命令行界面（feature: cli）
│   ├── mod.rs
│   └── commands.rs
├── gui/              # 图形界面（feature: gui）
│   ├── app.rs        # 应用入口与主循环
│   ├── state.rs      # 界面状态模型
│   ├── playback.rs   # 广告段预览播放
│   ├── menu.rs       # 工具栏与状态栏
│   ├── save.rs       # 保存（裁剪写盘）流程
│   ├── file_list.rs  # 左侧文件列表
│   ├── results.rs    # 时间轴与结果视图
│   └── fonts.rs      # 中文字体
└── ffi.rs            # C ABI 接口
```

## C ABI 接口

`mp3cutad_rs.dll` 导出以下函数（`ffi.rs`）：

| 函数 | 签名说明 |
|---|---|
| `mp3cutad_detect` | `(files_ptr, files_count, progress_cb, user_data) -> *mut CDetectionResult`，检测广告 |
| `mp3cutad_cut` | `(input, output, ranges_ptr, ranges_count, total_length) -> *mut c_char`，按检测结果裁剪 |
| `mp3cutad_free_result` | 释放检测结果 |
| `mp3cutad_free_string` | 释放返回的字符串 |
| `mp3cutad_check_ffmpeg` | 检查 FFmpeg 是否可用 |
| `mp3cutad_version` | 返回版本字符串 |

## 已实现的优化

- **多特征融合**：FFT + MFCC + Chroma 加权相似度
- **自适应阈值**：根据匹配得分分布动态调整
- **时序约束**：Viterbi 时序一致性过滤
- **多探针 LSH**：容错哈希查询
- **SIMD 优化**：x86_64 AVX2 余弦相似度
- **检测进度**：预处理与比对阶段均有细粒度进度回调

## 性能

原项目基准测试环境（10 个 30 分钟音频）下，Rust 版相比 C# 版约有 2–4 倍速度提升，内存占用约为一半。实际耗时会随文件数量、时长和 CPU 核心数变化。

## AI 辅助开发

本项目的 Rust 改造在 [Trae](https://www.trae.cn/) IDE 的编码智能体（TraeCode）辅助下完成，使用模型包括 **DeepSeek-V4.1-Flash** 与 **Kimi-K3**。智能体参与的工作包括：

- 代码结构重组（`core` / `audio` / `pipeline` / `cli` / `gui` / `ffi` 分层）
- 缺陷修复（缓存键冲突、广告区间计数、整段广告处理、播放状态竞争、滚动条遮挡）
- 多音频格式支持与拼接输出策略
- 检测进度、README 文档编写与更新

所有改动均由维护者审阅、编译与实测验证。

## 许可证

MIT License

## 致谢

- 原项目：[licstar/MP3CutAd](https://github.com/licstar/MP3CutAd)（C#），本项目的算法设计参考自该项目
- Rust 生态：`rustfft`、`ndarray`、`rayon`、`hound`、`eframe/egui`、`rodio` 等
