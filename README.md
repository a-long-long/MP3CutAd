# MP3CutAd-RS

MP3 广告检测与裁剪工具的 Rust 重写版本。通过音频指纹匹配检测多个 MP3 文件中的重复片段（广告），并自动裁剪。

## 功能特性

- **高性能**: 使用 Rust 编写，相比原 C# 版本有 2-4 倍速度提升
- **并行处理**: 自动利用多核 CPU 进行并行计算
- **精确检测**: 基于 FFT 频谱分析和 LSH 局部敏感哈希算法
- **易于使用**: 提供命令行界面，支持一键检测和裁剪

## 安装

### 前置要求

- Rust 1.70+ (如果使用 cargo 编译)
- FFmpeg (需要添加到系统 PATH)

### 编译

```bash
cargo build --release
```

编译后的可执行文件位于 `target/release/mp3cutad-rs.exe`

## 使用方法

### 1. 检查环境

```bash
mp3cutad-rs check
```

### 2. 检测广告

```bash
mp3cutad-rs detect file1.mp3 file2.mp3 file3.mp3 -o result.txt
```

### 3. 一键检测并裁剪

```bash
mp3cutad-rs auto -i input_dir -o output_dir
```

### 4. 分步操作

```bash
# 检测
mp3cutad-rs detect *.mp3 -o result.txt

# 裁剪
mp3cutad-rs cut -i input_dir -o output_dir -r result.txt
```

## 命令行参数

### `detect` - 检测广告

```
mp3cutad-rs detect [OPTIONS] <FILES>...

参数:
  <FILES>...    MP3 文件列表

选项:
  -o, --output <OUTPUT>    输出检测结果到文件
```

### `cut` - 裁剪广告

```
mp3cutad-rs cut [OPTIONS] -i <INPUT_DIR> -o <OUTPUT_DIR> -r <RESULT_FILE>

选项:
  -i, --input-dir <INPUT_DIR>      输入目录
  -o, --output-dir <OUTPUT_DIR>    输出目录
  -r, --result-file <RESULT_FILE>  检测结果文件
```

### `auto` - 一键操作

```
mp3cutad-rs auto [OPTIONS] -i <INPUT_DIR> -o <OUTPUT_DIR>

选项:
  -i, --input-dir <INPUT_DIR>      输入目录
  -o, --output-dir <OUTPUT_DIR>    输出目录
```

## 算法原理

1. **音频转换**: 使用 FFmpeg 将 MP3 转换为 WAV（单声道，22050Hz）
2. **频谱分析**: 对音频进行 FFT 变换，生成频谱图
3. **指纹哈希**: 使用 LSH (局部敏感哈希) 生成音频指纹
4. **相似度匹配**: 计算不同文件间指纹的余弦相似度
5. **区间合并**: 将相似的连续片段合并为广告区间
6. **裁剪拼接**: 移除广告区间，拼接剩余部分

## 性能对比

| 版本 | 处理速度 | 内存占用 | 准确率 |
|------|---------|---------|--------|
| C# 原版 | 1x | 1x | 基准 |
| Rust 版本 | 2-4x | 0.5x | 相同 |

测试环境: 10 个 30 分钟 MP3 文件
- C# 版本: ~45 秒
- Rust 版本: ~12 秒

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
│   ├── detect.rs     # 广告检测流程
│   └── range.rs      # 区间数据结构
├── audio/            # 音频 I/O
│   ├── ffmpeg.rs     # FFmpeg 封装
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

## 已实现的优化

- **多特征融合**: FFT + MFCC + Chroma 加权相似度
- **自适应阈值**: 根据匹配得分分布动态调整
- **时序约束**: Viterbi 时序一致性过滤
- **多探针 LSH**: 容错哈希查询
- **SIMD 优化**: x86_64 AVX2 余弦相似度

## 优化建议

1. **流式处理**: 对大文件使用内存映射 I/O
2. **GPU 加速**: 如仍需大规模批量处理，可按当前 `cudarc` API 重新引入

## 许可证

MIT License

## 致谢

本项目是 MP3CutAd (C# 版本) 的 Rust 重写，感谢原项目的算法设计。
