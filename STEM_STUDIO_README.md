# yyw

用于扫描 NCM 与已经存在的 MP3、FLAC、WAV 等音频，并通过 Demucs 做音源分离。这个 GUI 不执行 NCM 解密或转换；如果同名转换结果已经存在，会自动使用该音频继续处理。

## 运行

双击：

```text
run_stem_studio.bat
```

或在当前目录运行：

```powershell
python stem_studio.py
```

## 使用流程

1. 在“音频目录”里选择 NCM、MP3、FLAC、WAV 所在目录。
2. 点击“扫描”。
3. 如果列表里的 NCM 已经有同名 `.flac/.mp3/.wav` 等文件，会显示“已找到”并可继续处理。
4. 选择一首或多首可处理音频。
5. 选择分离模式：
   - `vocals`：输出人声和伴奏。
   - `four_stems`：输出人声、鼓、贝斯、其他。
   - `six_stems`：输出人声、鼓、贝斯、吉他、钢琴、其他。
6. 点击“分离选中”。
7. 在“输出音轨”里选择分离结果并播放。

## Demucs

本应用会尝试调用：

- `tools\demucs.exe`
- 系统 PATH 中的 `demucs`
- `python -m demucs`

如果没有安装 Demucs，界面可以打开，但分离任务会失败。安装 Demucs 后重新启动应用即可。

输出目录默认是：

```text
X:\wyy_tran\stems_output
```
