# MP3CutAd-RS

音频广告检测与裁剪工具。通过音频指纹匹配，找出多个音频文件中重复出现的片段（通常是广告），并自动裁剪掉这些片段。

> **项目来源**：本项目参考 GitHub 上的同名项目 [licstar/MP3CutAd](https://github.com/licstar/MP3CutAd)（C# 实现），在其算法思路基础上使用 **Rust 重写**，并重新组织了代码结构、补充了图形界面和多格式支持。

## 功能特性

- **多格式支持**：MP3 / WAV / FLAC / M4A / AAC / OGG / OPUS / WMA，裁剪输出保持源格式，无需先转成 MP3
- **图形界面**：文件列表、检测进度、可视化时间轴、广告段点击试听、多选与批量保存
- **结果可解释**：每个广告段显示匹配度，并按高/中/低置信度分色；可查看能量包络与 Mel 频谱，直接核对判定依据
- **结果排序**：文件列表可按平均匹配度从高到低排列，便于优先复核可疑结果
- **命令行**：支持检测、裁剪、一键处理，适合批量任务
- **C ABI 动态库**：便于其他语言或宿主程序集成
- **并行计算**：基于 Rayon 自动利用多核 CPU
- **结果缓存**：FFT 频谱与 LSH 指纹缓存到系统临时目录，重复检测时跳过解码与重算
- **精确检测**：FFT 频谱分析 + LSH 局部敏感哈希 + 多特征融合 + Viterbi 时序过滤
- **边界精修**：区间两端逐帧收紧（最多 5 帧），减少误切与漏切
- **块级置信度**：按 1 秒分块统计匹配比例，避免局部强匹配掩盖区间中间的断裂
- **HPSS 源分离（可选）**：谐波-打击乐分离后只对打击乐轨道检测，削弱背景音乐干扰
- **事件分类**：可用命令行训练随机森林模型，对音频按时间打标签（如纯音 / 噪声），结果在时间轴下方单独轨道展示
- **完整的使用文档**：图形界面逐项说明见 [docs/gui-usage.md](docs/gui-usage.md)

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
2. **检测广告**：状态栏显示实时进度（预处理音频 i/N、比对音频片段 i/M）；勾选「HPSS 分离」会在预处理阶段先做谐波-打击乐分离，只对打击乐轨道检测，适合广告段混有背景音乐的素材
3. **查看结果**：每个文件一条时间轴，按原始时长绘制，广告段以色块标注；运行过事件分类时，时间轴下方还会显示事件标签轨道
4. **试听**：点击色块播放该广告段，再次点击停止
5. **多选**：`Ctrl + 点击` 可多选广告段（黄色边框）
6. **保存**：可输出到目录或勾选“覆盖原文件”；覆盖时先裁剪到临时文件，成功后才替换原文件
7. **匹配度分色**：色块颜色表示该段的匹配度，标题行右侧有图例

   | 颜色 | 匹配度 | 含义 |
   |---|---|---|
   | 红 | ≥ 80% | 两段音频高度重合，通常为同一广告素材 |
   | 橙 | 60%–80% | 存在重复结构但有差异（音量、码率、版本或截断） |
   | 黄 | < 60% | 仅弱相似，误报风险最高，建议优先复核 |
   | 原配色 | 无分数 | 旧版结果文件或匹配度不可用 |

   匹配度只用于提示与排序，不参与裁剪决策，保存时仍会删除全部有效广告段。

8. **按匹配度排序**：勾选标题行「按匹配度排序」，文件按平均匹配度降序排列，无匹配度的文件排在最后
9. **频谱视图**：勾选「频谱」后点击任意音轨，在该音轨下方展开

   - **能量包络**：快速判断广告段是否落在有声区域（平线通常意味着误把静音判为广告）
   - **Mel 频谱（对数能量，低频在下）**：核对被判为重复的两段音频形态是否真的一致
   - 广告段位置以半透明红框叠加在图上，与时间轴对齐

   频谱直接读取检测阶段落盘的 FFT 缓存，不重新解码；若清理过缓存目录，需重新检测一次才能查看。

10. **阈值设置**：点击工具栏「阈值设置」打开窗口

   | 设置项 | 默认值 | 作用 |
   |---|---|---|
   | 最小广告长度 | 0 秒（不限） | 短于此长度的区间不显示、不参与裁剪 |
   | 最小出现次数 | 2 | 至少被多少个文件匹配到才算广告 |
   | 高置信度阈值 | 0.80 | 匹配度不低于此值显示为红色 |
   | 低置信度阈值 | 0.60 | 低于此值显示为黄色，介于两者之间为橙色 |

   修改立即生效（清空旧的选择以避免序号错位），可一键「恢复默认」；当前阈值会显示在底部状态栏。

11. **HPSS 分离**：工具栏勾选后，检测前先做谐波-打击乐分离，只保留打击乐轨道参与指纹比对。对旋律性强、背景音乐持续的广告效果更稳，代价是每个文件都要重新分离，无法复用 FFT 缓存

12. **事件分类**：工具栏「模型」选择 `.mcrf` 模型文件，再点「分类事件」对列表中所有文件按时间打标签

   - 每个事件在时间轴下方单独轨道展示，颜色按标签区分，标题行右侧有事件图例
   - 事件轨道与广告段轨道相互独立，可同时查看；分类不参与裁剪决策

13. **训练模型**：工具栏「🏋 训练模型」打开训练窗口

   - 输入标签后点「导出选中广告段...」，把当前文件、当前区间的广告段另存为独立音频并加入样本
   - 点「添加文件样本...」可批量加入整段音频（用于节目、音乐等负样本）
   - 样本列表显示「标签 + 文件名」，可逐条移除或一键清空
   - 调整树数量、最大深度、测试集比例后点「训练并保存模型...」，训练完成会自动加载该模型，可直接用于分类
   - 窗口底部显示各标签的精确率、召回率与 F1

   图形界面的完整控件说明、工作流与快捷键见 [docs/gui-usage.md](docs/gui-usage.md)。

## 命令行用法

### 检测广告

```bash
mp3cutad-cli detect file1.mp3 file2.flac -o result.txt
```

启用 HPSS 源分离（较慢，无缓存复用）：

```bash
mp3cutad-cli detect --hpss file1.mp3 file2.flac -o result.txt
```

### 裁剪（使用已有检测结果）

```bash
mp3cutad-cli cut -i input_dir -o output_dir -r result.txt
```

### 一键检测并裁剪

```bash
mp3cutad-cli auto -i input_dir -o output_dir
```

### 训练事件分类模型

也可以直接在图形界面中完成：工具栏「训练模型」→ 导出广告段 / 添加样本 → 训练并保存。

数据列表每行格式为 `音频路径 标签`，相对路径按列表文件所在目录解析；同一行可用「A到B」标注区间内多段：

```text
/data/speech_001.wav 纯音
/data/jingle_a.wav 噪声
/data/mixed.wav 纯音到噪声
```

```bash
mp3cutad-cli train -d data.txt -o model.mcrf --trees 100 --max-depth 32 --test-split 0.2
```

训练自动以 2 秒窗口、1 秒步长扩展样本，特征标准化后训练随机森林，并在留出的测试集上打印各标签的精确率、召回率与 F1。

### 用模型分类音频

```bash
mp3cutad-cli classify a.mp3 b.flac -m model.mcrf --confidence 0.55
```

输出每个文件的总帧数与检测到的事件区间（起止时间、标签、置信度）。`--window` / `--hop` 可覆盖模型内的窗口与步长（单位秒）。

### 参数说明

```
mp3cutad-cli detect [OPTIONS] <FILES>...
  <FILES>...                音频文件列表
  -o, --output <OUTPUT>     输出检测结果到文件
      --hpss                启用 HPSS 打击乐轨道源分离

mp3cutad-cli cut [OPTIONS] -i <INPUT_DIR> -o <OUTPUT_DIR> -r <RESULT_FILE>
  -i, --input-dir <DIR>     输入目录
  -o, --output-dir <DIR>    输出目录
  -r, --result-file <FILE>  检测结果文件

mp3cutad-cli auto [OPTIONS] -i <INPUT_DIR> -o <OUTPUT_DIR>
  -i, --input-dir <DIR>     输入目录
  -o, --output-dir <DIR>    输出目录

mp3cutad-cli train [OPTIONS] -d <DATA_LIST> -o <OUTPUT>
  -d, --data-list <FILE>    数据列表（每行 "路径 标签"）
  -o, --output <FILE>       输出的 .mcrf 模型文件
      --trees <N>           随机森林树数量，默认 100
      --max-depth <N>       单棵树最大深度，默认 32
      --test-split <R>      测试集比例，默认 0.2
      --seed <N>            随机种子，默认 42

mp3cutad-cli classify [OPTIONS] <FILES>...
  <FILES>...                音频文件列表
  -m, --model <FILE>        模型文件（必填）
  -w, --window <SEC>        覆盖窗口时长
      --hop <SEC>           覆盖窗口步长
  -c, --confidence <R>      置信度阈值，默认 0.55
```

### 退出与跳过规则

- 某个文件全部内容都被判定为广告时，该文件会报错跳过，不影响其余文件
- 检测结果只保留至少被 2 个文件匹配到的区间（`出现次数 >= 2`）；命令行固定为 2，图形界面可在「阈值设置」中调整最小出现次数、最小长度与置信度分色阈值

### 检测结果文件格式

`detect -o` 与 `cut -r` 使用纯文本格式：

```
<文件数>
<文件路径>
<总帧数>
<区间数>
<begin> <end> <count> <type_id> <score>
...
```

- 时间单位为 0.1 秒（帧），10 帧 = 1 秒
- `count` 为出现次数，`type_id` 为聚类编号
- `score` 为匹配度（0.0–1.0），按区间内 1 秒分块统计：达标块取相似度、未达标块按 0 计，再求均值
- 旧版结果文件缺少 `score` 列时按 0 处理，界面显示为原配色

## 算法原理

1. **解码**：调用 FFmpeg 将源音频转为单声道 22050Hz WAV
2. **频谱分析**：分帧做 FFT，得到频谱图
3. **特征提取**：FFT 频谱 + MFCC + Chroma 多特征
4. **指纹哈希**：LSH（局部敏感哈希）生成音频指纹，支持多探针查询
5. **相似度匹配**：文件两两比较，计算指纹余弦相似度（x86_64 下使用 AVX2）
6. **时序过滤**：Viterbi 算法保证匹配区间在时间上连续一致
7. **边界精修**：在已定位的区间两端逐帧比对，向内收缩最多 5 帧（0.5 秒），收紧起止点
8. **块级置信度**：按 1 秒分块统计区间内的相似度，未达标块按 0 计后取均值，得到"整段有多大比例、有多像"的匹配度（仅用于展示与排序）
9. **区间合并**：合并连续匹配片段，得到广告区间
10. **裁剪拼接**：移除广告区间，拼接剩余片段并保持原格式

### 检测参数

以下常量集中在 [src/core/detect.rs](src/core/detect.rs) 顶部，可按素材特点调整：

| 常量 | 默认值 | 作用 |
|---|---|---|
| `HASH_WINDOW_SIZE` | 20 帧（2 秒） | LSH 指纹窗口长度 |
| `MIN_THRESHOLD` / `MAX_THRESHOLD` | 0.45 / 0.80 | 自适应阈值的下限与上限 |
| `FINE_TUNE_THRESHOLD` | 0.65 | 区间扩展与块级置信度的达标线 |
| `FINE_TUNE_WINDOW` | 4 帧 | 精细比对窗口 |
| `POSITION_SEARCH_RANGE` | 5 帧 | 起点位置搜索范围 |
| `BOUNDARY_TIGHTEN_THRESHOLD` | 0.75 | 区间尾部收缩阈值 |
| `BOUNDARY_REFINE_WINDOW` | 2 帧 | 边界精修比对窗口 |
| `BOUNDARY_REFINE_THRESHOLD` | 0.80 | 边界精修收缩阈值 |
| `BOUNDARY_REFINE_MAX_SHIFT` | 5 帧 | 单侧最大收缩量 |
| `CONFIDENCE_BLOCK` | 10 帧（1 秒） | 块级置信度分块粒度 |

调整这些常量会改变检测结果，建议先用少量样本对比调整前后的区间与匹配度；块级置信度只影响展示与排序，不影响裁剪决策。

### HPSS 源分离（可选）

启用后在第 1 步与第 2 步之间插入一次分离，其余步骤不变：

1. 对 22050Hz 单声道采样做 STFT（`n_fft=2048`，`hop=1024`）
2. 分别沿时间轴（窗口 17）与频率轴（窗口 11）做中值滤波，得到谐波分量与打击乐分量
3. 用软掩码按能量比例分配频谱，保留 STFT 相位做逆变换回时域
4. 只把打击乐轨道写回临时 WAV 并继续做 FFT / 哈希

人声与音乐的持续音更偏谐波，广告中的音效、节奏点更偏打击乐，因此只比对打击乐轨道能降低背景音乐造成的假相似。代价是每次检测都要重新分离，不复用 FFT 与哈希缓存。参数定义在 [src/core/separation.rs](src/core/separation.rs)（`HpssParams`）。

### 事件分类

事件分类与广告检测相互独立，是一条可选的旁路：

1. 把音频转为 22050Hz 单声道，按窗口（默认 2 秒）与步长（默认 1 秒）切片
2. 每段提取 128 维段级特征（40 维 Mel、13 维 MFCC、6 波段谱对比度及其统计量，`n_fft=2048`）
3. 用训练时保存的均值/标准差做标准化，送入随机森林（默认 100 棵树、最大深度 32）
4. 置信度低于阈值（默认 0.55）的窗口标记为「无事件」，再经多数投票平滑（窗口 3）
5. 合并相邻同标签窗口，输出事件区间（起止帧、标签、平均置信度）

模型以 `.mcrf` 保存（`MCRF` 魔数 + 版本号 + bincode 数据），加载时会校验特征维度、标签数、树数与节点总数，避免损坏文件导致崩溃。相关实现见 [segfeat.rs](src/core/segfeat.rs)、[forest.rs](src/core/forest.rs)、[model.rs](src/core/model.rs)、[classify.rs](src/core/classify.rs)。

## 输出格式与缓存

**输出格式策略**（裁剪拼接时）：

| 格式 | 策略 |
|---|---|
| MP3 / WAV | 流拷贝（`-c copy`），无重编码、无质量损失 |
| FLAC | 无损重编码，保证时长元数据正确 |
| OGG / OPUS / M4A / AAC / WMA | 重编码一次（流拷贝拼接易导致时间戳与时长错误） |

**缓存**：检测中间结果（FFT 频谱图与 LSH 哈希）缓存在系统临时目录下的 `mp3cutad_rs`，以完整路径 + 文件修改时间校验，文件变更或缓存损坏会自动重算；解码产生的临时 WAV 在 FFT 计算完成后即删除。启用 HPSS 时结果与常规缓存不可混用，因此不读也不写 FFT/哈希缓存。删除整个缓存目录不影响使用，只是下次需要重新计算。

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
│   ├── detect.rs     # 广告检测流程（含进度回调、HPSS 开关）
│   ├── range.rs      # 区间数据结构
│   ├── separation.rs # HPSS 谐波-打击乐源分离
│   ├── segfeat.rs    # 事件分类的段级特征（Mel/MFCC/谱对比度）
│   ├── forest.rs     # 随机森林训练与预测
│   ├── model.rs      # .mcrf 模型序列化与校验
│   └── classify.rs   # 滑窗分类、平滑与事件合并
├── audio/            # 音频 I/O
│   ├── ffmpeg.rs     # FFmpeg 封装（解码 / 裁剪 / 拼接）
│   └── cache.rs      # 检测中间结果缓存
├── pipeline.rs       # 检测 → 裁剪的流程编排
├── result_io.rs      # 检测结果文件读写
├── training.rs       # 事件分类模型训练（CLI / GUI 共用）
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
│   ├── spectrogram.rs # 能量包络与 Mel 频谱可视化
│   ├── settings.rs   # 阈值设置窗口
│   ├── train.rs      # 训练样本管理与模型训练窗口
│   └── fonts.rs      # 中文字体
└── ffi.rs            # C ABI 接口

docs/
└── gui-usage.md      # 图形界面完整使用说明
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
- **结果可解释**：块级匹配度、置信度分色、能量包络与 Mel 频谱、按匹配度排序
- **检测精度**：亚秒级边界精修、块级置信度（参考 pHash 的块级 BER 思路）
- **抗背景音乐**：可选 HPSS 打击乐轨道分离，降低音乐造成的假相似
- **事件分类**：随机森林 + 段级特征，支持命令行训练、GUI 可视化标签轨道
- **界面内训练**：从广告段导出样本、补充负样本、训练并自动加载模型，全程无需命令行
- **模型安全**：`.mcrf` 带魔数与版本号，加载时校验维度、树数与节点总数

## 性能

原项目基准测试环境（10 个 30 分钟音频）下，Rust 版相比 C# 版约有 2–4 倍速度提升，内存占用约为一半。实际耗时会随文件数量、时长和 CPU 核心数变化。

## AI 辅助开发

本项目的 Rust 改造在 [Trae](https://www.trae.cn/) IDE 的编码智能体（TraeCode）辅助下完成，使用模型包括 **DeepSeek-V4.1-Flash** 与 **Kimi-K3**。智能体参与的工作包括：

- 代码结构重组（`core` / `audio` / `pipeline` / `cli` / `gui` / `ffi` 分层）
- 缺陷修复（缓存键冲突、广告区间计数、整段广告处理、播放状态竞争、滚动条遮挡）
- 多音频格式支持与拼接输出策略
- 结果可解释性（区间匹配度、置信度分色、能量包络与 Mel 频谱、按匹配度排序）
- 检测精度（块级置信度、亚秒级边界精修）
- HPSS 源分离与随机森林事件分类（含模型文件格式与 GUI 标签轨道）
- 检测进度、README 与 GUI 使用文档编写与更新

所有改动均由维护者审阅、编译与实测验证。

## 许可证

MIT License

## 致谢

- 原项目：[licstar/MP3CutAd](https://github.com/licstar/MP3CutAd)（C#），本项目的算法设计参考自该项目
- Rust 生态：`rustfft`、`ndarray`、`rayon`、`hound`、`serde`/`bincode`、`rand`、`eframe/egui`、`rodio` 等
