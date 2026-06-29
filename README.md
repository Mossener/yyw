# yyw

yyw 是一个本地音频处理和分轨试听工具。它可以扫描网易云下载目录中的 `.ncm` 和常见音频文件，转换 NCM，调用 Demucs 等分离器生成分轨，并在图形界面中直接播放输入音频和输出音轨。

项目提供两个入口：

- `yyw.exe`：图形界面，适合日常使用。
- `yyw-cli.exe`：命令行入口，适合批处理和自动化。

## 主要功能

- 扫描 `.ncm`、`.flac`、`.mp3`、`.wav`、`.m4a` 等音频文件。
- 使用 `tools\ncmdump.exe` 转换网易云 `.ncm` 文件。
- 调用 Demucs 或 `separators.json` 中配置的其他分离器生成分轨。
- 在 UI 中查看输入音频和输出音轨。
- 播放输入音频或分离后的音轨，支持暂停、停止、进度显示和拖动跳转。
- 显示封面和歌词，支持同名图片、同名歌词、内嵌封面和内嵌歌词。
- 右键曲目或音轨查看媒体信息，例如文件大小、时长、码率、编码、采样率和声道。
- 使用 FFmpeg 将原音频元数据迁移到输出分轨。

## 快速启动

在项目目录中直接运行：

```powershell
.\yyw.exe
```

也可以双击根目录的 `yyw.exe`。

命令行入口：

```powershell
.\yyw-cli.exe --help
```

源码根目录不再保留启动用 `.bat` 脚本。发布包中可以包含 `start_yyw.bat`，用于检查 Python、安装 Demucs/torchcodec 并启动 `yyw.exe`。

## GUI 使用流程

1. 打开 `yyw.exe`。
2. 在“音频目录”选择包含音频文件的目录。
3. 在“输出目录”选择分轨结果保存目录，默认是 `stems_output`。
4. 点击“扫描”。
5. 在左侧选择要处理的输入音频。
6. 选择分离工具、分离模式、模型和设备。
7. 点击“转换 NCM”或“分离选中”。
8. 在右侧“输出音轨”卡片列表中选择结果。
9. 双击音轨卡片直接播放，或点击底部播放器的“播放音轨”。

常用模式：

- `vocals`：输出人声和伴奏。
- `four_stems`：输出人声、鼓、贝斯、其他。
- `six_stems`：输出人声、鼓、贝斯、吉他、钢琴、其他。

## 播放和查看信息

底部播放器支持：

- 播放输入音频。
- 播放输出音轨。
- 暂停、继续和停止。
- 显示当前时间和总时长。
- 拖动进度条跳转。
- 显示封面和歌词。

右键输入曲目或输出音轨可以：

- 播放。
- 查看详细信息。
- 打开所在目录。

歌词支持普通 LRC，也会自动清理网易云扩展 JSON 歌词行，只显示可读文本。

## CLI 使用

查看帮助：

```powershell
.\yyw-cli.exe --help
```

示例：

```powershell
# 分离单个音频
.\yyw-cli.exe -i ".\song.mp3" -o ".\stems_output" -M vocals

# 只转换 NCM，不做分离
.\yyw-cli.exe -i ".\song.ncm" --convert-only

# 批量扫描目录并使用六轨模型
.\yyw-cli.exe -i "G:\CloudMusic\VipSongsDownload" -o ".\stems_output" -M six_stems -m htdemucs_6s
```

主要参数：

```text
-i, --input <INPUT>      输入文件或目录
-o, --output <OUTPUT>    输出目录
--tool <TOOL>            分离工具名称，对应 separators.json
-m, --model <MODEL>      模型名
-M, --mode <MODE>        分离模式
-d, --device <DEVICE>    设备：auto / cpu / cuda
--convert-only           仅转换 NCM
```

## 依赖和工具

### ncmdump

NCM 转换需要：

```text
tools\ncmdump.exe
```

缺少该文件时，普通音频仍可扫描和分离，但 `.ncm` 不能自动转换。

### Demucs

默认分离器是 Demucs。程序会按以下位置查找：

1. 程序目录或父目录中的 `tools\demucs.exe`。
2. 程序目录或父目录中的 `runtime\python\python.exe -m demucs`。
3. PATH 中的 `demucs.exe`。
4. `D:\conda\python.exe -m demucs`。
5. `python -m demucs`。

如果需要手动安装 Demucs，可以在你使用的 Python 环境中执行：

```powershell
pip install demucs torchcodec
```

### FFmpeg

媒体信息读取、封面提取、时长兜底和元数据迁移需要：

```text
tools\ffmpeg.exe
```

建议同时放置 FFmpeg 运行所需 DLL。当前项目的 `tools` 目录已经包含 FFmpeg 相关文件。

程序会从 exe 所在目录、父目录、当前目录、PATH 等位置查找工具，因此从根目录或 `target\release` 启动都能找到根目录下的 `tools`。

## 配置文件

### stem_studio_settings.json

保存最近使用的音频目录、输出目录、模型、模式、设备和分离工具。该文件包含本机路径，通常不需要提交。

### separators.json

配置可选分离器。默认包含 Demucs、Spleeter 和 UVR 示例。

示例：

```json
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
```

占位符：

- `{model}`：替换为界面或 CLI 中选择的模型。
- `{output}`：替换为输出目录。

## 从源码构建

需要 Rust 工具链。

开发运行：

```powershell
cargo run
```

构建 release：

```powershell
cargo build --release
```

构建输出：

```text
target\release\yyw.exe
target\release\yyw-cli.exe
```

根目录的 `yyw.exe` 和 `yyw-cli.exe` 是便于直接启动的构建产物副本。

## 目录结构

```text
wyy_tran\
├── src\
│   ├── main.rs        # GUI 入口，基于 egui/eframe
│   ├── lib.rs         # 扫描、转换、分离和工具查找逻辑
│   └── bin\cli.rs     # CLI 入口，基于 clap
├── tools\
│   ├── ncmdump.exe    # NCM 转换工具
│   ├── ffmpeg.exe     # 媒体信息、封面、时长和元数据辅助处理
│   └── *.dll          # FFmpeg 运行库
├── separators.json    # 分离器配置
├── radian.png         # 应用图标来源
├── build.rs           # Windows exe 图标资源构建脚本
├── stems_output\      # 默认输出目录
├── yyw.exe            # GUI 程序
└── yyw-cli.exe        # CLI 程序
```

## 常见问题

### 右侧看不到输出音轨

点击“刷新输出”，并确认输出目录是实际保存分轨的目录。右侧音轨以卡片形式显示，双击卡片即可播放。

### NCM 没有自动转换

确认 `tools\ncmdump.exe` 存在。转换后的 `.flac` 或 `.mp3` 通常会放在源 `.ncm` 所在目录。

### 找不到 Demucs

确认当前 Python 环境中能运行：

```powershell
python -m demucs --help
```

或者在 `separators.json` 中把 Demucs 命令改成你本机的实际路径。

### 封面或媒体信息不显示

确认 `tools\ffmpeg.exe` 和相关 DLL 存在。程序会优先读取同名图片和音频内嵌封面；如果音频没有封面，就会显示未找到封面。
