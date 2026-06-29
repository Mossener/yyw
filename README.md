# yyw

yyw 是一个本地音频处理工具，用于批量扫描网易云 `.ncm` 文件和常见音频文件，并调用外部分离器生成伴奏、人声、鼓、贝斯等音轨。

它提供两个入口：

- **GUI**：适合日常扫描、转换、分离和试听。
- **CLI**：适合批处理、脚本调用和自动化流程。

## 功能

- **NCM 转换**：调用 `tools\ncmdump.exe` 将 `.ncm` 转成可处理的音频文件。
- **音轨分离**：默认调用 Demucs，也可以通过 `separators.json` 接入 Spleeter、UVR 或其他命令行分离器。
- **批量处理**：支持扫描目录中的 `.ncm`、`.flac`、`.mp3`、`.wav` 等文件。
- **本地试听**：GUI 内置播放器，可直接播放输入音频和分离后的音轨。
- **进度显示**：GUI 底部显示当前任务状态和总体进度。
- **元数据迁移**：CLI 可在存在 `ffmpeg.exe` 时尝试把原音频元数据迁移到输出音轨。

## 快速启动

如果当前目录已有可执行文件，直接运行：

```powershell
.\yyw.exe
```

也可以双击：

```text
run.bat
```

`run.bat` 会先激活 `D:\conda` 环境，再启动 `yyw.exe`。如果你使用的是便携 Python 运行时，可以改用：

```text
run_portable.bat
```

## 从源码运行

开发环境需要 Rust 工具链。

```powershell
cargo run
```

构建 release：

```powershell
cargo build --release
```

构建完成后 GUI 程序位于：

```text
target\release\yyw.exe
```

CLI 程序位于：

```text
target\release\yyw-cli.exe
```

## GUI 使用流程

1. 打开 `yyw.exe`。
2. 在“音频目录”中选择包含 `.ncm` 或音频文件的目录。
3. 在“输出目录”中选择分离结果保存位置，默认是 `stems_output`。
4. 点击“扫描”。
5. 选择需要处理的输入音频。
6. 选择“工具”“分离模式”“模型”和“设备”。
7. 点击“转换 NCM”或“分离选中”。
8. 在“输出音轨”列表中选择结果并试听。

常用模式：

- `vocals`：输出人声和伴奏。
- `four_stems`：输出人声、鼓、贝斯、其他。
- `six_stems`：输出人声、鼓、贝斯、吉他、钢琴、其他。

## CLI 使用

当前目录中如果已有 `yyw-cli.exe`，可以直接使用：

```powershell
.\yyw-cli.exe --help
```

从源码构建后，CLI 输出为 `target\release\yyw-cli.exe`：

```powershell
.\target\release\yyw-cli.exe --help
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

## 依赖

### ncmdump

NCM 转换依赖：

```text
tools\ncmdump.exe
```

如果缺少该文件，普通音频仍可分离，但 `.ncm` 不能自动转换。

### 分离器

默认配置使用 Demucs。程序会尝试使用以下路径之一：

1. `tools\demucs.exe`
2. `runtime\python\python.exe -m demucs`
3. PATH 中的 `demucs.exe`
4. `D:\conda\python.exe -m demucs`
5. `python -m demucs`

如果没有安装 Demucs，可以先运行：

```powershell
setup_light.bat
```

或手动安装：

```powershell
pip install demucs torchcodec
```

`setup_light.bat` 会尝试安装 Demucs、torchcodec，并下载 FFmpeg 相关 DLL。

### FFmpeg

CLI 的元数据迁移功能依赖：

```text
tools\ffmpeg.exe
```

缺少 FFmpeg 不影响基本分离，只会跳过元数据迁移。

## 配置文件

### stem_studio_settings.json

程序会自动生成并保存最近使用的目录、模型、模式、设备和分离工具。该文件包含本机路径，通常不需要提交到版本库。

### separators.json

`separators.json` 用来配置可选分离器。默认包含 Demucs、Spleeter 和 UVR 示例。

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

## 目录结构

```text
wyy_tran\
├── src\
│   ├── main.rs        # GUI 入口，基于 egui/eframe
│   ├── lib.rs         # CLI 复用的扫描、转换、分离逻辑
│   └── bin\cli.rs     # CLI 入口，基于 clap
├── tools\
│   ├── ncmdump.exe    # NCM 转换工具
│   └── ffmpeg.exe     # 元数据迁移和音频辅助处理
├── separators.json    # 分离器配置
├── run.bat
├── run_portable.bat
├── setup_light.bat
├── stem_studio.py     # 早期 Python 版本，保留作参考
└── stems_output\      # 默认输出目录
```

## 常见问题

### 启动后找不到 Demucs

确认 Demucs 是否安装在当前 Python 环境中：

```powershell
python -m demucs --help
```

如果使用 `run.bat`，确认 `D:\conda` 存在且其中安装了 Demucs。

### NCM 没有自动转换

确认 `tools\ncmdump.exe` 存在。转换后的 `.flac` 或 `.mp3` 会优先放在源 `.ncm` 所在目录。

### 输出目录没有新音轨

检查所选模式和模型是否匹配。例如 `six_stems` 通常应使用 `htdemucs_6s`。
