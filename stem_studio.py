from __future__ import annotations

import json
import os
import queue
import shutil
import subprocess
import threading
import time
from ctypes import create_unicode_buffer, windll
from dataclasses import dataclass
from pathlib import Path
from tkinter import filedialog, messagebox
import tkinter as tk
from tkinter import ttk


APP_NAME = "Stem Studio"
DEFAULT_SOURCE = r"G:\CloudMusic\VipSongsDownload"
DEFAULT_OUTPUT = Path.cwd() / "stems_output"
SETTINGS_FILE = Path.cwd() / "stem_studio_settings.json"

CONVERTED_EXTENSIONS = {".flac", ".mp3", ".wav", ".m4a", ".aac", ".ogg", ".wma", ".aiff", ".aif"}
PROTECTED_EXTENSIONS = {".ncm"}
INPUT_EXTENSIONS = CONVERTED_EXTENSIONS | PROTECTED_EXTENSIONS
PLAYABLE_EXTENSIONS = {".mp3", ".wav", ".wma", ".aiff", ".aif"}
STEM_EXTENSIONS = {".wav", ".mp3", ".flac", ".m4a", ".aac", ".ogg", ".wma"}


@dataclass
class AudioItem:
    path: Path
    status: str = "待处理"
    process_path: Path | None = None
    kind: str = "audio"

    @property
    def can_separate(self) -> bool:
        return self.process_path is not None


class MciPlayer:
    def __init__(self) -> None:
        self.alias = "stem_studio_player"
        self.current: Path | None = None
        self.paused = False

    def _send(self, command: str) -> str:
        buffer = create_unicode_buffer(512)
        error = windll.winmm.mciSendStringW(command, buffer, 511, 0)
        if error:
            error_buffer = create_unicode_buffer(512)
            windll.winmm.mciGetErrorStringW(error, error_buffer, 511)
            raise RuntimeError(error_buffer.value or f"MCI error {error}")
        return buffer.value

    def close(self) -> None:
        try:
            self._send(f"close {self.alias}")
        except RuntimeError:
            pass
        self.current = None
        self.paused = False

    def play(self, path: Path) -> None:
        self.close()
        safe_path = str(path.resolve()).replace('"', "")
        self._send(f'open "{safe_path}" alias {self.alias}')
        self._send(f"play {self.alias}")
        self.current = path
        self.paused = False

    def pause_or_resume(self) -> bool:
        if not self.current:
            return False
        if self.paused:
            self._send(f"resume {self.alias}")
            self.paused = False
        else:
            self._send(f"pause {self.alias}")
            self.paused = True
        return self.paused

    def stop(self) -> None:
        if self.current:
            self._send(f"stop {self.alias}")
        self.close()


class StemStudio(tk.Tk):
    def __init__(self) -> None:
        super().__init__()
        self.title(APP_NAME)
        self.geometry("1180x760")
        self.minsize(980, 640)

        self.items: dict[str, AudioItem] = {}
        self.log_queue: queue.Queue[str] = queue.Queue()
        self.active_process: subprocess.Popen[str] | None = None
        self.player = MciPlayer()

        self.settings = self._load_settings()
        self.source_var = tk.StringVar(value=self.settings.get("source", DEFAULT_SOURCE))
        self.output_var = tk.StringVar(value=self.settings.get("output", str(DEFAULT_OUTPUT)))
        self.model_var = tk.StringVar(value=self.settings.get("model", "htdemucs"))
        self.mode_var = tk.StringVar(value=self.settings.get("mode", "vocals"))
        self.device_var = tk.StringVar(value=self.settings.get("device", "auto"))
        self.status_var = tk.StringVar(value="准备就绪")
        self.tool_var = tk.StringVar(value="")
        self.now_playing_var = tk.StringVar(value="未播放")
        self.progress_var = tk.DoubleVar(value=0)

        self._configure_style()
        self._build_ui()
        self._refresh_tool_status()
        self._pump_logs()
        self.protocol("WM_DELETE_WINDOW", self._on_close)

    def _configure_style(self) -> None:
        self.configure(bg="#f5f7f8")
        style = ttk.Style(self)
        style.theme_use("clam")
        style.configure(".", font=("Microsoft YaHei UI", 10), background="#f5f7f8", foreground="#1e2328")
        style.configure("TFrame", background="#f5f7f8")
        style.configure("Header.TFrame", background="#1e2328")
        style.configure("Header.TLabel", background="#1e2328", foreground="#ffffff", font=("Microsoft YaHei UI", 16, "bold"))
        style.configure("MutedHeader.TLabel", background="#1e2328", foreground="#cad5df", font=("Microsoft YaHei UI", 9))
        style.configure("TLabel", background="#f5f7f8", foreground="#1e2328")
        style.configure("Muted.TLabel", background="#f5f7f8", foreground="#697783", font=("Microsoft YaHei UI", 9))
        style.configure("TButton", padding=(12, 7), borderwidth=0)
        style.configure("Accent.TButton", background="#006d77", foreground="#ffffff")
        style.map("Accent.TButton", background=[("active", "#005f68")], foreground=[("active", "#ffffff")])
        style.configure("Danger.TButton", background="#a6402f", foreground="#ffffff")
        style.map("Danger.TButton", background=[("active", "#893528")], foreground=[("active", "#ffffff")])
        style.configure("Treeview", rowheight=29, background="#ffffff", fieldbackground="#ffffff", borderwidth=0)
        style.configure("Treeview.Heading", background="#e4eaee", foreground="#1e2328", font=("Microsoft YaHei UI", 9, "bold"))
        style.configure("Horizontal.TProgressbar", background="#006d77", troughcolor="#dbe4e8")

    def _build_ui(self) -> None:
        header = ttk.Frame(self, style="Header.TFrame", padding=(18, 15, 18, 13))
        header.pack(fill=tk.X)
        ttk.Label(header, text=APP_NAME, style="Header.TLabel").pack(anchor=tk.W)
        ttk.Label(
            header,
            text="扫描 NCM 与已存在的 FLAC/MP3/WAV；找到同名可处理音频后可继续分离音轨",
            style="MutedHeader.TLabel",
        ).pack(anchor=tk.W, pady=(4, 0))

        root = ttk.Frame(self, padding=16)
        root.pack(fill=tk.BOTH, expand=True)

        paths = ttk.Frame(root)
        paths.pack(fill=tk.X)
        self._path_row(paths, "音频目录", self.source_var, self._browse_source, 0)
        self._path_row(paths, "输出目录", self.output_var, self._browse_output, 1)

        controls = ttk.Frame(root)
        controls.pack(fill=tk.X, pady=(12, 10))

        ttk.Label(controls, text="分离模式").pack(side=tk.LEFT)
        mode_box = ttk.Combobox(
            controls,
            textvariable=self.mode_var,
            values=("vocals", "four_stems", "six_stems"),
            width=12,
            state="readonly",
        )
        mode_box.pack(side=tk.LEFT, padx=(6, 12))

        ttk.Label(controls, text="模型").pack(side=tk.LEFT)
        model_box = ttk.Combobox(
            controls,
            textvariable=self.model_var,
            values=("htdemucs", "htdemucs_ft", "mdx_extra", "htdemucs_6s"),
            width=14,
        )
        model_box.pack(side=tk.LEFT, padx=(6, 12))

        ttk.Label(controls, text="设备").pack(side=tk.LEFT)
        device_box = ttk.Combobox(
            controls,
            textvariable=self.device_var,
            values=("auto", "cpu", "cuda"),
            width=8,
            state="readonly",
        )
        device_box.pack(side=tk.LEFT, padx=(6, 12))

        ttk.Button(controls, text="扫描", style="Accent.TButton", command=self.scan_inputs).pack(side=tk.LEFT)
        ttk.Button(controls, text="分离选中", command=self.separate_selected).pack(side=tk.LEFT, padx=(8, 0))
        ttk.Button(controls, text="转换 NCM", command=self.convert_ncm_selected).pack(side=tk.LEFT, padx=(8, 0))
        ttk.Button(controls, text="停止任务", style="Danger.TButton", command=self.stop_task).pack(side=tk.LEFT, padx=(8, 0))
        ttk.Button(controls, text="刷新输出", command=self.scan_stems).pack(side=tk.LEFT, padx=(8, 0))
        ttk.Button(controls, text="打开输出目录", command=self.open_output_dir).pack(side=tk.LEFT, padx=(8, 0))

        ttk.Label(root, textvariable=self.tool_var, style="Muted.TLabel").pack(fill=tk.X, pady=(0, 8))

        panes = ttk.PanedWindow(root, orient=tk.HORIZONTAL)
        panes.pack(fill=tk.BOTH, expand=True)

        left = ttk.Frame(panes)
        right = ttk.Frame(panes)
        panes.add(left, weight=2)
        panes.add(right, weight=1)

        ttk.Label(left, text="输入音频").pack(anchor=tk.W)
        self.input_tree = ttk.Treeview(left, columns=("type", "status", "path"), show="tree headings", selectmode="extended")
        self.input_tree.heading("#0", text="文件")
        self.input_tree.heading("type", text="类型")
        self.input_tree.heading("status", text="状态")
        self.input_tree.heading("path", text="路径")
        self.input_tree.column("#0", width=280, minwidth=180)
        self.input_tree.column("type", width=80, anchor=tk.CENTER, stretch=False)
        self.input_tree.column("status", width=130, minwidth=100)
        self.input_tree.column("path", width=360, minwidth=220)
        self.input_tree.pack(fill=tk.BOTH, expand=True, pady=(6, 0))

        right_top = ttk.Frame(right)
        right_top.pack(fill=tk.BOTH, expand=True)
        ttk.Label(right_top, text="输出音轨").pack(anchor=tk.W)
        self.stem_tree = ttk.Treeview(right_top, columns=("stem", "path"), show="tree headings", selectmode="browse")
        self.stem_tree.heading("#0", text="歌曲")
        self.stem_tree.heading("stem", text="音轨")
        self.stem_tree.heading("path", text="路径")
        self.stem_tree.column("#0", width=210, minwidth=150)
        self.stem_tree.column("stem", width=100, anchor=tk.CENTER, stretch=False)
        self.stem_tree.column("path", width=320, minwidth=180)
        self.stem_tree.pack(fill=tk.BOTH, expand=True, pady=(6, 0))

        log_frame = ttk.Frame(right)
        log_frame.pack(fill=tk.BOTH, expand=True, pady=(12, 0))
        ttk.Label(log_frame, text="任务日志").pack(anchor=tk.W)
        self.log_text = tk.Text(log_frame, height=9, wrap=tk.WORD, borderwidth=0, bg="#ffffff", fg="#1e2328")
        self.log_text.pack(fill=tk.BOTH, expand=True, pady=(6, 0))

        footer = ttk.Frame(self, padding=(16, 12))
        footer.pack(fill=tk.X)
        ttk.Button(footer, text="播放输入", command=self.play_input).pack(side=tk.LEFT)
        ttk.Button(footer, text="播放音轨", style="Accent.TButton", command=self.play_stem).pack(side=tk.LEFT, padx=(8, 0))
        ttk.Button(footer, text="暂停/继续", command=self.pause_or_resume).pack(side=tk.LEFT, padx=(8, 0))
        ttk.Button(footer, text="停止播放", style="Danger.TButton", command=self.stop_playback).pack(side=tk.LEFT, padx=(8, 0))
        ttk.Label(footer, textvariable=self.now_playing_var).pack(side=tk.LEFT, padx=(12, 0))

        status = ttk.Frame(self, padding=(16, 0, 16, 12))
        status.pack(fill=tk.X)
        ttk.Progressbar(status, variable=self.progress_var, maximum=100).pack(side=tk.LEFT, fill=tk.X, expand=True)
        ttk.Label(status, textvariable=self.status_var, width=36, anchor=tk.E).pack(side=tk.LEFT, padx=(12, 0))

    def _path_row(self, parent: ttk.Frame, label: str, variable: tk.StringVar, command, row: int) -> None:
        ttk.Label(parent, text=label, width=8).grid(row=row, column=0, sticky=tk.W, pady=4)
        ttk.Entry(parent, textvariable=variable).grid(row=row, column=1, sticky=tk.EW, padx=(8, 8), pady=4)
        ttk.Button(parent, text="浏览", command=command).grid(row=row, column=2, sticky=tk.E, pady=4)
        parent.columnconfigure(1, weight=1)

    def _load_settings(self) -> dict[str, str]:
        if not SETTINGS_FILE.exists():
            return {}
        try:
            return json.loads(SETTINGS_FILE.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}

    def _save_settings(self) -> None:
        data = {
            "source": self.source_var.get(),
            "output": self.output_var.get(),
            "model": self.model_var.get(),
            "mode": self.mode_var.get(),
            "device": self.device_var.get(),
        }
        SETTINGS_FILE.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")

    def _browse_source(self) -> None:
        selected = filedialog.askdirectory(initialdir=self.source_var.get() or str(Path.home()))
        if selected:
            self.source_var.set(selected)
            self._save_settings()
            self.scan_inputs()

    def _browse_output(self) -> None:
        selected = filedialog.askdirectory(initialdir=self.output_var.get() or str(Path.cwd()))
        if selected:
            self.output_var.set(selected)
            self._save_settings()
            self.scan_stems()

    def _demucs_base_command(self) -> list[str]:
        local = Path.cwd() / "tools" / "demucs.exe"
        if local.exists():
            return [str(local)]
        demucs = shutil.which("demucs")
        if demucs:
            return [demucs]
        return ["python", "-m", "demucs"]

    def _ncmdump_base_command(self) -> list[str]:
        local = Path.cwd() / "tools" / "ncmdump.exe"
        if local.exists():
            return [str(local)]
        ncmdump = shutil.which("ncmdump")
        if ncmdump:
            return [ncmdump]
        return []

    def _refresh_tool_status(self) -> None:
        command = self._demucs_base_command()
        if command[:2] == ["python", "-m"]:
            text = "Demucs：未发现 demucs.exe，将尝试使用 python -m demucs"
        else:
            text = f"Demucs：{command[0]}"
        ncm_cmd = self._ncmdump_base_command()
        self.ncmdump_available = bool(ncm_cmd)
        if ncm_cmd:
            text += f"  |  ncmdump：{ncm_cmd[0]}"
        else:
            text += "  |  ncmdump：未找到（NCM 无法自动转换）"
        self.tool_var.set(text)

    def scan_inputs(self) -> None:
        self._save_settings()
        source = Path(self.source_var.get()).expanduser()
        self.items.clear()
        for row in self.input_tree.get_children():
            self.input_tree.delete(row)

        if not source.exists():
            self.status_var.set("音频目录不存在")
            return

        files = [path for path in source.rglob("*") if path.is_file() and path.suffix.lower() in INPUT_EXTENSIONS]
        files.sort(key=lambda item: item.name.lower())

        for path in files:
            iid = str(path)
            audio = self._make_audio_item(path, source)
            self.items[iid] = audio
            process_text = str(audio.process_path) if audio.process_path and audio.process_path != audio.path else str(path)
            self.input_tree.insert(
                "",
                tk.END,
                iid=iid,
                text=path.name,
                values=(path.suffix.lower().lstrip(".").upper(), audio.status, process_text),
            )

        ready = sum(1 for item in self.items.values() if item.can_separate)
        self.status_var.set(f"已扫描 {len(files)} 个文件，{ready} 个可分离")
        self.scan_stems()

    def _make_audio_item(self, path: Path, source_root: Path) -> AudioItem:
        if path.suffix.lower() not in PROTECTED_EXTENSIONS:
            return AudioItem(path=path, process_path=path)

        converted = self._find_converted_audio(path, source_root)
        if converted:
            return AudioItem(
                path=path,
                status=f"已找到 {converted.suffix.lower().lstrip('.').upper()}",
                process_path=converted,
                kind="ncm",
            )
        return AudioItem(path=path, status="NCM：等待外部转换", process_path=None, kind="ncm")

    def _find_converted_audio(self, ncm_path: Path, source_root: Path) -> Path | None:
        for extension in CONVERTED_EXTENSIONS:
            candidate = ncm_path.with_suffix(extension)
            if candidate.exists():
                return candidate

        matches = [
            path
            for path in source_root.rglob(f"{ncm_path.stem}.*")
            if path.is_file() and path.suffix.lower() in CONVERTED_EXTENSIONS
        ]
        matches.sort(key=lambda item: (item.parent != ncm_path.parent, item.suffix.lower(), item.name.lower()))
        return matches[0] if matches else None

    def _convert_ncm(self, ncm_path: Path, output_dir: Path) -> Path | None:
        command = self._ncmdump_base_command()
        if not command:
            return None
        cmd = command + ["-o", str(output_dir), str(ncm_path)]
        try:
            result = subprocess.run(
                cmd, capture_output=True, text=True, encoding="utf-8", errors="replace"
            )
            if result.stdout:
                self._append_log(result.stdout)
            if result.returncode != 0:
                self._append_log(f"ncmdump 返回错误码 {result.returncode}\n{result.stderr}\n")
                return None
            for ext in (".flac", ".mp3"):
                candidate = output_dir / (ncm_path.stem + ext)
                if candidate.exists():
                    return candidate
            return None
        except FileNotFoundError:
            self._append_log("ncmdump 未找到\n")
            return None
        except Exception as exc:
            self._append_log(f"ncmdump 执行异常: {exc}\n")
            return None

    def scan_stems(self) -> None:
        output = Path(self.output_var.get()).expanduser()
        for row in self.stem_tree.get_children():
            self.stem_tree.delete(row)

        if not output.exists():
            self.status_var.set("输出目录还不存在")
            return

        stems = [path for path in output.rglob("*") if path.is_file() and path.suffix.lower() in STEM_EXTENSIONS]
        stems.sort(key=lambda item: (item.parent.name.lower(), item.name.lower()))

        for path in stems:
            track_name = path.parent.name
            stem_name = path.stem
            self.stem_tree.insert("", tk.END, iid=str(path), text=track_name, values=(stem_name, str(path)))

        self.status_var.set(f"已找到 {len(stems)} 个输出音轨")

    def separate_selected(self) -> None:
        if self.active_process and self.active_process.poll() is None:
            self.status_var.set("已有任务正在运行")
            return

        selected = list(self.input_tree.selection())
        if not selected:
            self.status_var.set("请先选择音频")
            return

        self._save_settings()
        thread = threading.Thread(target=self._run_demucs_batch, args=(selected,), daemon=True)
        thread.start()

    def convert_ncm_selected(self) -> None:
        if self.active_process and self.active_process.poll() is None:
            self.status_var.set("已有任务正在运行")
            return

        selected = list(self.input_tree.selection())
        if not selected:
            self.status_var.set("请先选择 NCM 文件")
            return

        ncm_items = [iid for iid in selected if self.items.get(iid) and self.items[iid].kind == "ncm"]
        if not ncm_items:
            self.status_var.set("选中项中没有 NCM 文件")
            return

        self._save_settings()
        thread = threading.Thread(target=self._run_ncm_convert_batch, args=(ncm_items,), daemon=True)
        thread.start()

    def _run_ncm_convert_batch(self, iids: list[str]) -> None:
        total = len(iids)
        completed = 0
        failed = 0

        for index, iid in enumerate(iids, start=1):
            item = self.items.get(iid)
            if not item:
                continue

            self._set_input_status(iid, "转换中...")
            self._ui_status(f"转换 NCM {index}/{total}: {item.path.name}")
            self._append_log(f"\n=== {item.path.name} ===\n正在转换 NCM...\n")

            converted = self._convert_ncm(item.path, item.path.parent)
            if converted:
                item.process_path = converted
                item.status = f"已转换 {converted.suffix}"
                completed += 1
                self._set_input_status(iid, f"已转换 {converted.suffix}", str(converted))
                self._append_log(f"转换成功：{converted.name}\n")
            else:
                failed += 1
                self._set_input_status(iid, "转换失败")
                self._append_log("转换失败\n")

            self.after(0, self.progress_var.set, index / total * 100)

        self._ui_status(f"转换完成 {completed}，失败 {failed}")
        self.after(0, self.scan_stems)

    def _run_demucs_batch(self, iids: list[str]) -> None:
        output = Path(self.output_var.get()).expanduser()
        output.mkdir(parents=True, exist_ok=True)
        total = len(iids)
        completed = 0
        failed = 0

        for index, iid in enumerate(iids, start=1):
            item = self.items.get(iid)
            if not item:
                continue

            if item.kind == "ncm" and not item.can_separate:
                if not self.ncmdump_available:
                    self._set_input_status(iid, "跳过：未找到 ncmdump")
                    self._append_log(
                        f"\n=== {item.path.name} ===\n"
                        "未找到 ncmdump，无法自动转换 NCM。请将 ncmdump.exe 放到 tools 目录。\n"
                    )
                    self.after(0, self.progress_var.set, index / total * 100)
                    continue

                self._set_input_status(iid, "转换 NCM...")
                self._ui_status(f"转换 NCM {index}/{total}: {item.path.name}")
                self._append_log(f"\n=== {item.path.name} ===\n正在自动转换 NCM...\n")

                converted = self._convert_ncm(item.path, item.path.parent)
                if converted:
                    item.process_path = converted
                    item.status = f"已转换 {converted.suffix}"
                    self._set_input_status(
                        iid, f"已转换 {converted.suffix}", str(converted)
                    )
                    self._append_log(f"NCM 转换成功：{converted.name}\n")
                else:
                    self._set_input_status(iid, "NCM 转换失败")
                    self._append_log("NCM 转换失败\n")
                    failed += 1
                    self.after(0, self.progress_var.set, index / total * 100)
                    continue

            if not item.can_separate:
                self._set_input_status(iid, "跳过：无可处理音频")
                self._append_log(f"\n=== {item.path.name} ===\n未找到同名 FLAC/MP3/WAV，已跳过。\n")
                self.after(0, self.progress_var.set, index / total * 100)
                continue

            self._set_input_status(iid, "处理中")
            assert item.process_path is not None
            self._ui_status(f"分离 {index}/{total}: {item.process_path.name}")
            self._append_log(f"\n=== {item.path.name} ===\n使用音频：{item.process_path}\n")

            command = self._build_demucs_command(item.process_path, output)
            self._append_log(" ".join(f'"{part}"' if " " in part else part for part in command) + "\n")

            try:
                demucs_env = os.environ.copy()
                demucs_env.setdefault("PYTHONIOENCODING", "utf-8")
                demucs_env.setdefault("PYTHONUTF8", "1")
                self.active_process = subprocess.Popen(
                    command,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    encoding="utf-8",
                    errors="replace",
                    env=demucs_env,
                )
                assert self.active_process.stdout is not None
                for line in self.active_process.stdout:
                    self.log_queue.put(line)
                exit_code = self.active_process.wait()
                if exit_code == 0:
                    completed += 1
                    self._set_input_status(iid, "已完成")
                else:
                    failed += 1
                    self._set_input_status(iid, f"失败 {exit_code}")
            except FileNotFoundError:
                failed += 1
                self._set_input_status(iid, "未安装 Demucs")
                self._append_log("未找到 Demucs。可安装 demucs，或将 demucs.exe 放到 tools 目录。\n")
                break
            except Exception as exc:
                failed += 1
                self._set_input_status(iid, "失败")
                self._append_log(f"{exc}\n")

            self.active_process = None
            self.after(0, self.progress_var.set, index / total * 100)

        self._ui_status(f"分离完成 {completed}，失败 {failed}")
        self.after(0, self.scan_stems)

    def _build_demucs_command(self, audio_path: Path, output: Path) -> list[str]:
        model = self.model_var.get().strip() or "htdemucs"
        mode = self.mode_var.get()
        device = self.device_var.get()

        if mode == "six_stems" and model != "htdemucs_6s":
            model = "htdemucs_6s"

        command = self._demucs_base_command()
        command.extend(["-n", model, "-o", str(output)])

        if mode == "vocals":
            command.append("--two-stems=vocals")
        if device != "auto":
            command.extend(["-d", device])

        command.append(str(audio_path))
        return command

    def _set_input_status(self, iid: str, status: str, process_path: str | None = None) -> None:
        def update() -> None:
            item = self.items.get(iid)
            if item:
                item.status = status
            values = list(self.input_tree.item(iid, "values"))
            if len(values) >= 3:
                values[1] = status
                if process_path is not None:
                    values[2] = process_path
                self.input_tree.item(iid, values=values)

        self.after(0, update)

    def stop_task(self) -> None:
        if not self.active_process or self.active_process.poll() is not None:
            self.status_var.set("没有正在运行的任务")
            return
        self.active_process.terminate()
        self.status_var.set("已请求停止任务")

    def _append_log(self, text: str) -> None:
        self.log_queue.put(text)

    def _pump_logs(self) -> None:
        try:
            while True:
                text = self.log_queue.get_nowait()
                self.log_text.insert(tk.END, text)
                self.log_text.see(tk.END)
        except queue.Empty:
            pass
        self.after(120, self._pump_logs)

    def _ui_status(self, text: str) -> None:
        self.after(0, self.status_var.set, text)

    def _play_path(self, path: Path) -> None:
        if path.suffix.lower() not in PLAYABLE_EXTENSIONS:
            messagebox.showinfo(APP_NAME, f"当前播放器可能不支持 {path.suffix}，请优先播放分离后的 WAV 或 MP3。")
        try:
            self.player.play(path)
            self.now_playing_var.set(f"正在播放：{path.name}")
            self.status_var.set("播放中")
        except Exception as exc:
            messagebox.showerror(APP_NAME, f"播放失败：\n{exc}")
            self.status_var.set("播放失败")

    def play_input(self) -> None:
        selected = self.input_tree.selection()
        if not selected:
            self.status_var.set("请先选择输入音频")
            return
        item = self.items.get(selected[0])
        if item and item.process_path:
            self._play_path(item.process_path)
        elif item:
            self.status_var.set("这个 NCM 还没有同名可播放音频")

    def play_stem(self) -> None:
        selected = self.stem_tree.selection()
        if not selected:
            self.status_var.set("请先选择输出音轨")
            return
        self._play_path(Path(selected[0]))

    def pause_or_resume(self) -> None:
        try:
            paused = self.player.pause_or_resume()
            self.status_var.set("已暂停" if paused else "播放中")
        except Exception as exc:
            messagebox.showerror(APP_NAME, f"播放控制失败：\n{exc}")

    def stop_playback(self) -> None:
        self.player.stop()
        self.now_playing_var.set("未播放")
        self.status_var.set("已停止")

    def open_output_dir(self) -> None:
        output = Path(self.output_var.get()).expanduser()
        output.mkdir(parents=True, exist_ok=True)
        os.startfile(output)

    def _on_close(self) -> None:
        self._save_settings()
        self.stop_playback()
        if self.active_process and self.active_process.poll() is None:
            self.active_process.terminate()
        self.destroy()


def main() -> None:
    app = StemStudio()
    app.scan_inputs()
    while True:
        try:
            app.mainloop()
            break
        except KeyboardInterrupt:
            app._on_close()
            time.sleep(0.1)


if __name__ == "__main__":
    main()
