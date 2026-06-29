# Stem Studio

NCM 解密 + 音轨分离工具。支持 GUI 和 CLI 两种模式。

## 功能

- **NCM 解密** — 调用 ncmdump 自动将 `.ncm` 转为 `.flac` / `.mp3`
- **音轨分离** — 调用 Demucs / 可配置的分离器，提取人声、鼓、贝斯等音轨
- **试听** — GUI 内置播放器，支持选中音频/音轨直接试听

## 截图

```
┌──────────────────────────────────────────────┐
│ Stem Studio      [扫描][分离][转NCM][停止]…   │
├──────────────────────────────────────────────┤
│ 音频目录 [___________________________] [浏览] │
│ 输出目录 [___________________________] [浏览] │
│ 工具 [Demucs] 模式 [vocals] 模型 [htdemucs]  │
├────────────────────┬─────────────────────────┤
│ 输入音频 (表格)     │ 输出音轨 (表格)          │
│                    │ 任务日志                 │
├────────────────────┴─────────────────────────┤
│ [▶输入][▶音轨][暂停][停止]  进度条  状态      │
└──────────────────────────────────────────────┘
```

## 安装

### 前置依赖

```bash
pip install demucs torchcodec
```

FFmpeg DLL 需放入 `Lib\site-packages\torchcodec\`：
[下载 ffmpeg-release-full-shared.7z](https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-full-shared.7z)，解压后把 `bin\av*.dll` `bin\sw*.dll` 复制到 torchcodec 目录。

或一键运行 `setup_light.bat` 自动完成。

### 编译

```bash
cargo build --release
```

产物：
- `target/release/stem-studio.exe` — GUI
- `target/release/cli.exe` — 命令行

## 使用

### GUI

双击 `run_stem_studio.bat`，或直接运行 `stem-studio.exe`。

### CLI

```bash
stem-cli -i song.mp3 -o output/ --mode vocals

# 只转 NCM
stem-cli -i song.ncm --convert-only

# 目录批量 + 六轨分离
stem-cli -i "G:\CloudMusic\VipSongsDownload" -o stems/ -M six_stems -m htdemucs_6s
```

```
Usage: stem-cli [OPTIONS]

  -i, --input <INPUT>    输入文件或目录
  -o, --output <OUTPUT>  输出目录
  --tool <TOOL>          分离器 (separators.json)
  -m, --model <MODEL>    模型名
  -M, --mode <MODE>      模式 (vocals/four_stems/six_stems)
  -d, --device <DEVICE>  设备 (auto/cpu/cuda)
  --convert-only         仅转换 NCM, 不分离
```

## 配置

`separators.json` — 可自定义分离器：

```json
[
  {
    "name": "Demucs",
    "command": ["python", "-m", "demucs"],
    "models": ["htdemucs", "htdemucs_ft", "mdx_extra", "htdemucs_6s"],
    "modes": ["vocals", "four_stems", "six_stems"],
    "args_before": ["-n", "{model}", "-o", "{output}"],
    "two_stem_flag": "--two-stems=vocals",
    "device_flag": "-d",
    "stems_mode": "flag"
  }
]
```

按模板添加新条目即可接入自定义分离器（如 UVR、Spleeter 等）。

## 目录结构

```
wyy_tran/
├── src/
│   ├── lib.rs        # 共享核心
│   ├── main.rs       # GUI (egui)
│   └── bin/cli.rs    # CLI (clap)
├── tools/
│   └── ncmdump.exe   # NCM 解密
├── separators.json   # 分离器配置
├── run_stem_studio.bat
├── setup_light.bat
└── stem_studio.py    # 遗留 Python 版
```

## 技术栈

| 层 | 技术 |
|----|------|
| 语言 | Rust |
| GUI | egui + eframe |
| 音频 | rodio (symphonia) |
| 分离 | Demucs (Python 子进程) |
| 解密 | ncmdump (C++ 子进程) |
