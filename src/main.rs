#![windows_subsystem = "windows"]

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use walkdir::WalkDir;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const APP_NAME: &str = "yyw";
const SETTINGS_FILE: &str = "stem_studio_settings.json";
const DEFAULT_SOURCE: &str = r"G:\CloudMusic\VipSongsDownload";
const DEFAULT_OUTPUT: &str = "stems_output";
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

const CONVERTED_EXTS: &[&str] = &[
    "flac", "mp3", "wav", "m4a", "aac", "ogg", "wma", "aiff", "aif",
];
const INPUT_EXTS: &[&str] = &[
    "flac", "mp3", "wav", "m4a", "aac", "ogg", "wma", "aiff", "aif", "ncm",
];
const STEM_EXTS: &[&str] = &["wav", "mp3", "flac", "m4a", "aac", "ogg", "wma"];

type ArcBool = Arc<Mutex<bool>>;

#[derive(Debug, Clone, Deserialize)]
struct SeparatorConfig {
    name: String,
    command: Vec<String>,
    models: Vec<String>,
    modes: Vec<String>,
    args_before: Vec<String>,
    two_stem_flag: String,
    device_flag: String,
    #[serde(default)]
    stems_mode: String,
}

fn load_separators() -> Vec<SeparatorConfig> {
    if let Ok(data) = std::fs::read_to_string("separators.json") {
        if let Ok(p) = serde_json::from_str::<Vec<SeparatorConfig>>(&data) {
            if !p.is_empty() {
                return p;
            }
        }
    }
    vec![SeparatorConfig {
        name: "Demucs".into(),
        command: vec!["python".into(), "-m".into(), "demucs".into()],
        models: vec![
            "htdemucs".into(),
            "htdemucs_ft".into(),
            "mdx_extra".into(),
            "htdemucs_6s".into(),
        ],
        modes: vec!["vocals".into(), "four_stems".into(), "six_stems".into()],
        args_before: vec![
            "-n".into(),
            "{model}".into(),
            "-o".into(),
            "{output}".into(),
        ],
        two_stem_flag: "--two-stems=vocals".into(),
        device_flag: "-d".into(),
        stems_mode: "flag".into(),
    }]
}

fn find_separator<'a>(configs: &'a [SeparatorConfig], name: &str) -> Option<&'a SeparatorConfig> {
    configs.iter().find(|c| c.name == name)
}

#[derive(Clone, Copy)]
struct ProgressScope {
    start: f32,
    span: f32,
}

#[derive(Serialize, Deserialize, Clone)]
struct Settings {
    source: String,
    output: String,
    model: String,
    mode: String,
    device: String,
    separator: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: DEFAULT_SOURCE.to_string(),
            output: PathBuf::from(DEFAULT_OUTPUT).to_string_lossy().to_string(),
            model: "htdemucs".to_string(),
            mode: "vocals".to_string(),
            device: "auto".to_string(),
            separator: "Demucs".to_string(),
        }
    }
}

#[derive(Clone, PartialEq)]
enum AudioKind {
    Normal,
    Ncm,
}

#[derive(Clone)]
struct AudioItem {
    path: PathBuf,
    status: String,
    process_path: Option<PathBuf>,
    kind: AudioKind,
}

impl AudioItem {
    fn can_separate(&self) -> bool {
        self.process_path.is_some()
    }
}

#[derive(Clone)]
struct StemItem {
    path: PathBuf,
    track_name: String,
    stem_name: String,
}

enum TaskMessage {
    #[allow(dead_code)]
    Log(String),
    Progress(f32),
    Status(String),
    InputStatus(usize, String, Option<String>),
    Done,
}

struct StemStudio {
    source_dir: String,
    output_dir: String,
    model: String,
    mode: String,
    device: String,
    separator: String,
    separators: Vec<SeparatorConfig>,

    ncmdump_path: Option<PathBuf>,
    demucs_command: Vec<String>,
    ncmdump_available: bool,
    tool_status: String,

    items: Vec<AudioItem>,
    stems: Vec<StemItem>,
    selected_items: HashSet<usize>,
    selected_stem: Option<usize>,

    progress: f32,
    status: String,

    task_running: ArcBool,
    task_receiver: Option<Receiver<TaskMessage>>,

    _stream: Option<OutputStream>,
    stream_handle: Option<OutputStreamHandle>,
    sink: Option<Sink>,
    now_playing: String,
    paused: bool,
}

fn status_color(status: &str) -> egui::Color32 {
    if status.contains("已完成")
        || status.contains("已找到")
        || status.contains("已转换")
        || status.contains("成功")
    {
        egui::Color32::from_rgb(74, 222, 128)
    } else if status.contains("失败") || status.contains("未找到") || status.contains("错误")
    {
        egui::Color32::from_rgb(239, 68, 68)
    } else if status.contains("处理中") || status.contains("转换中") {
        egui::Color32::from_rgb(250, 204, 21)
    } else {
        egui::Color32::from_rgb(148, 163, 184)
    }
}

impl StemStudio {
    fn new() -> Self {
        let settings = Self::load_settings();
        let separators = load_separators();
        let ncmdump_path = Self::find_tool("ncmdump.exe");
        let demucs_command = Self::find_demucs();
        let ncmdump_available = ncmdump_path.is_some();
        let tool_status =
            Self::build_tool_status(&demucs_command, ncmdump_available, &ncmdump_path);

        let (stream, stream_handle) = OutputStream::try_default()
            .unwrap_or_else(|_| OutputStream::try_default().expect("no audio output device"));

        Self {
            source_dir: settings.source,
            output_dir: settings.output,
            model: settings.model,
            mode: settings.mode,
            device: settings.device,
            separator: settings.separator,
            separators,
            ncmdump_path,
            demucs_command,
            ncmdump_available,
            tool_status,
            items: Vec::new(),
            stems: Vec::new(),
            selected_items: HashSet::new(),
            selected_stem: None,
            progress: 0.0,
            status: "准备就绪".to_string(),
            task_running: Arc::new(Mutex::new(false)),
            task_receiver: None,
            _stream: Some(stream),
            stream_handle: Some(stream_handle),
            sink: None,
            now_playing: "未播放".to_string(),
            paused: false,
        }
    }

    fn load_settings() -> Settings {
        let path = Path::new(SETTINGS_FILE);
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(s) = serde_json::from_str(&data) {
                return s;
            }
        }
        Settings::default()
    }

    fn save_settings(&self) {
        let data = serde_json::to_string_pretty(&Settings {
            source: self.source_dir.clone(),
            output: self.output_dir.clone(),
            model: self.model.clone(),
            mode: self.mode.clone(),
            device: self.device.clone(),
            separator: self.separator.clone(),
        })
        .unwrap_or_default();
        let _ = std::fs::write(SETTINGS_FILE, data);
    }

    fn find_tool(name: &str) -> Option<PathBuf> {
        if let Ok(cwd) = std::env::current_dir() {
            let local = cwd.join("tools").join(name);
            if local.exists() {
                return Some(local);
            }
        }
        if let Ok(paths) = std::env::var("PATH") {
            for dir in std::env::split_paths(&paths) {
                let c = dir.join(name);
                if c.exists() {
                    return Some(c);
                }
            }
        }
        None
    }

    fn find_demucs() -> Vec<String> {
        if let Ok(cwd) = std::env::current_dir() {
            let local = cwd.join("tools").join("demucs.exe");
            if local.exists() {
                return vec![local.to_string_lossy().to_string()];
            }
            // portable Python bundled with the app
            let portable = cwd.join("runtime").join("python").join("python.exe");
            if portable.exists() {
                return vec![
                    portable.to_string_lossy().to_string(),
                    "-m".into(),
                    "demucs".into(),
                ];
            }
        }
        if Self::find_tool("demucs.exe").is_some() {
            return vec!["demucs".to_string()];
        }
        let conda_python = PathBuf::from(r"D:\conda\python.exe");
        if conda_python.exists() {
            return vec![
                conda_python.to_string_lossy().to_string(),
                "-m".to_string(),
                "demucs".to_string(),
            ];
        }
        vec!["python".to_string(), "-m".to_string(), "demucs".to_string()]
    }

    fn build_tool_status(demucs: &[String], ncm_avail: bool, ncm_path: &Option<PathBuf>) -> String {
        let mut text = if demucs.first().map(|s| s.as_str()) == Some("python") {
            "Demucs: 未发现 demucs.exe, 将尝试使用 python -m demucs".to_string()
        } else {
            format!("Demucs: {}", demucs.first().unwrap())
        };
        if ncm_avail {
            text.push_str(&format!(
                "  |  ncmdump: {}",
                ncm_path.as_ref().unwrap().display()
            ));
        } else {
            text.push_str("  |  ncmdump: 未找到 (NCM 无法自动转换)");
        }
        text
    }

    fn scan_inputs(&mut self) {
        self.save_settings();
        self.items.clear();
        self.selected_items.clear();
        let source = Path::new(&self.source_dir);
        if !source.exists() {
            self.status = "音频目录不存在".to_string();
            return;
        }

        let mut files: Vec<PathBuf> = Vec::new();
        for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension() {
                    if INPUT_EXTS.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
                        files.push(entry.path().to_path_buf());
                    }
                }
            }
        }
        files.sort_by(|a, b| {
            a.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase()
                .cmp(
                    &b.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_lowercase(),
                )
        });

        for path in files {
            let is_ncm = path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase() == "ncm")
                .unwrap_or(false);
            let item = if is_ncm {
                self.make_ncm_item(&path, source)
            } else {
                AudioItem {
                    path: path.clone(),
                    status: "待处理".into(),
                    process_path: Some(path.clone()),
                    kind: AudioKind::Normal,
                }
            };
            self.items.push(item);
        }
        let ready = self.items.iter().filter(|i| i.can_separate()).count();
        self.status = format!("已扫描 {} 个文件, {} 个可分离", self.items.len(), ready);
        self.scan_stems();
    }

    fn make_ncm_item(&self, ncm_path: &Path, source_root: &Path) -> AudioItem {
        for ext in CONVERTED_EXTS {
            let c = ncm_path.with_extension(ext);
            if c.exists() {
                return AudioItem {
                    path: ncm_path.to_path_buf(),
                    status: format!("已找到 {}", ext.to_uppercase()),
                    process_path: Some(c),
                    kind: AudioKind::Ncm,
                };
            }
        }
        let stem = ncm_path.file_stem().unwrap_or_default().to_string_lossy();
        let mut cands: Vec<PathBuf> = Vec::new();
        for entry in WalkDir::new(source_root).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension() {
                    if CONVERTED_EXTS.contains(&ext.to_string_lossy().to_lowercase().as_str())
                        && entry
                            .path()
                            .file_stem()
                            .map(|s| s == stem.as_ref())
                            .unwrap_or(false)
                    {
                        cands.push(entry.path().to_path_buf());
                    }
                }
            }
        }
        cands.sort_by(|a, b| {
            (b.parent() == ncm_path.parent())
                .cmp(&(a.parent() == ncm_path.parent()))
                .then_with(|| {
                    a.extension()
                        .unwrap_or_default()
                        .cmp(b.extension().unwrap_or_default())
                })
        });
        if let Some(c) = cands.into_iter().next() {
            AudioItem {
                path: ncm_path.to_path_buf(),
                status: format!(
                    "已找到 {}",
                    c.extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_uppercase()
                ),
                process_path: Some(c),
                kind: AudioKind::Ncm,
            }
        } else {
            AudioItem {
                path: ncm_path.to_path_buf(),
                status: "NCM: 等待外部转换".into(),
                process_path: None,
                kind: AudioKind::Ncm,
            }
        }
    }

    fn scan_stems(&mut self) {
        self.stems.clear();
        let output = Path::new(&self.output_dir);
        if !output.exists() {
            return;
        }
        let mut found: Vec<StemItem> = Vec::new();
        for entry in WalkDir::new(output).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() {
                if let Some(ext) = entry.path().extension() {
                    if STEM_EXTS.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
                        found.push(StemItem {
                            track_name: entry
                                .path()
                                .parent()
                                .and_then(|p| p.file_name())
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            stem_name: entry
                                .path()
                                .file_stem()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default(),
                            path: entry.path().to_path_buf(),
                        });
                    }
                }
            }
        }
        found.sort_by(|a, b| {
            a.track_name
                .to_lowercase()
                .cmp(&b.track_name.to_lowercase())
                .then_with(|| a.stem_name.to_lowercase().cmp(&b.stem_name.to_lowercase()))
        });
        self.stems = found;
    }

    fn command_line(cmd: &[String]) -> String {
        cmd.iter()
            .map(|p| {
                if p.contains(' ') || p.contains('\'') || p.contains('"') {
                    format!("\"{}\"", p.replace('"', "\\\""))
                } else {
                    p.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn parse_progress_percent(text: &str) -> Option<f32> {
        let percent_pos = text.find('%')?;
        let before = &text[..percent_pos];
        let reversed: String = before
            .chars()
            .rev()
            .skip_while(|c| c.is_whitespace())
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        if reversed.is_empty() {
            return None;
        }
        let value: String = reversed.chars().rev().collect();
        value
            .parse::<f32>()
            .ok()
            .map(|p| (p / 100.0).clamp(0.0, 1.0))
    }

    fn progress_value(scope: ProgressScope, local: f32) -> f32 {
        (scope.start + scope.span * local.clamp(0.0, 1.0)).clamp(0.0, 1.0)
    }

    fn hide_child_window(command: &mut Command) {
        #[cfg(windows)]
        {
            command.creation_flags(CREATE_NO_WINDOW);
        }
    }

    fn add_python_runtime_paths(
        env: &mut std::collections::HashMap<String, String>,
        command: &str,
    ) {
        let exe = Path::new(command);
        if exe
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case("python.exe"))
            .unwrap_or(false)
        {
            if let Some(root) = exe.parent() {
                let paths = [
                    root.to_path_buf(),
                    root.join("Scripts"),
                    root.join("Library").join("bin"),
                ];
                let prefix = std::env::join_paths(paths.iter().filter(|p| p.exists()))
                    .ok()
                    .and_then(|p| p.into_string().ok())
                    .unwrap_or_default();
                if !prefix.is_empty() {
                    let path = env.entry("PATH".into()).or_default();
                    if path.is_empty() {
                        *path = prefix;
                    } else {
                        *path = format!("{prefix};{path}");
                    }
                }
            }
        }
    }

    fn send_output_line(
        sender: &Sender<TaskMessage>,
        source: &str,
        line: &str,
        progress: Option<ProgressScope>,
    ) {
        let text = line.trim();
        if text.is_empty() {
            return;
        }
        let _ = sender.send(TaskMessage::Log(format!("[{source}] {text}\n")));
        if let (Some(scope), Some(p)) = (progress, Self::parse_progress_percent(text)) {
            let _ = sender.send(TaskMessage::Progress(Self::progress_value(scope, p)));
        }
    }

    fn spawn_output_reader<R: Read + Send + 'static>(
        mut reader: R,
        source: &'static str,
        sender: Sender<TaskMessage>,
        progress: Option<ProgressScope>,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut pending = String::new();
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = String::from_utf8_lossy(&buf[..n]);
                        for ch in chunk.chars() {
                            if ch == '\n' || ch == '\r' {
                                Self::send_output_line(&sender, source, &pending, progress);
                                pending.clear();
                            } else {
                                pending.push(ch);
                            }
                        }
                        if pending.len() > 1200 {
                            Self::send_output_line(&sender, source, &pending, progress);
                            pending.clear();
                        }
                    }
                    Err(e) => {
                        let _ = sender
                            .send(TaskMessage::Log(format!("[{source}] 读取输出失败: {e}\n")));
                        break;
                    }
                }
            }
            Self::send_output_line(&sender, source, &pending, progress);
        })
    }

    fn run_command_streaming(
        cmd: &[String],
        env: Option<&std::collections::HashMap<String, String>>,
        sender: &Sender<TaskMessage>,
        running: ArcBool,
        progress: Option<ProgressScope>,
    ) -> Result<ExitStatus, String> {
        if cmd.is_empty() {
            return Err("命令为空".into());
        }
        let _ = sender.send(TaskMessage::Log(format!("$ {}\n", Self::command_line(cmd))));

        let mut command = Command::new(&cmd[0]);
        command
            .args(&cmd[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        Self::hide_child_window(&mut command);
        if let Some(env) = env {
            command.envs(env);
        }

        let mut child = command.spawn().map_err(|e| format!("启动命令失败: {e}"))?;
        let mut readers = Vec::new();
        if let Some(stdout) = child.stdout.take() {
            readers.push(Self::spawn_output_reader(
                stdout,
                "stdout",
                sender.clone(),
                progress,
            ));
        }
        if let Some(stderr) = child.stderr.take() {
            readers.push(Self::spawn_output_reader(
                stderr,
                "stderr",
                sender.clone(),
                progress,
            ));
        }

        let mut stop_sent = false;
        let status = loop {
            if !*running.lock().unwrap() && !stop_sent {
                let _ = child.kill();
                let _ = sender.send(TaskMessage::Log("任务已停止，正在终止当前命令...\n".into()));
                stop_sent = true;
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(100)),
                Err(e) => return Err(format!("等待命令失败: {e}")),
            }
        };

        for reader in readers {
            let _ = reader.join();
        }
        let _ = sender.send(TaskMessage::Log(format!(
            "退出码: {}\n",
            status.code().map_or_else(|| "无".into(), |c| c.to_string())
        )));
        Ok(status)
    }

    fn convert_ncm_streaming(
        ncmdump: &Path,
        ncm_path: &Path,
        out_dir: &Path,
        sender: &Sender<TaskMessage>,
        running: ArcBool,
        progress: Option<ProgressScope>,
    ) -> Result<PathBuf, String> {
        let cmd = vec![
            ncmdump.to_string_lossy().to_string(),
            "-o".to_string(),
            out_dir.to_string_lossy().to_string(),
            ncm_path.to_string_lossy().to_string(),
        ];
        let status = Self::run_command_streaming(&cmd, None, sender, running, progress)?;
        if !status.success() {
            return Err(format!(
                "ncmdump 返回错误码 {}",
                status.code().unwrap_or(-1)
            ));
        }
        let stem = ncm_path.file_stem().unwrap_or_default().to_string_lossy();
        for ext in CONVERTED_EXTS {
            let c = out_dir.join(format!("{}.{}", stem, ext));
            if c.exists() {
                return Ok(c);
            }
        }
        Err("ncmdump 未生成输出文件".into())
    }

    // ── background ──

    fn run_demucs_batch(
        items: Vec<AudioItem>,
        indices: Vec<usize>,
        output_dir: PathBuf,
        ncmdump_avail: bool,
        ncmdump_path: Option<PathBuf>,
        demucs_base: Vec<String>,
        separators: Vec<SeparatorConfig>,
        model: String,
        sep_name: String,
        mode: String,
        device: String,
        sender: Sender<TaskMessage>,
        running: ArcBool,
    ) {
        let total = indices.len();
        let (mut ok, mut fail) = (0u32, 0u32);
        for (round, (idx, mut item)) in indices.into_iter().zip(items).enumerate() {
            if !*running.lock().unwrap() {
                break;
            }
            let r = round as u32 + 1;
            let file_start = round as f32 / total as f32;
            let file_span = 1.0 / total as f32;
            let mut converted_this_item = false;

            if item.kind == AudioKind::Ncm && !item.can_separate() {
                if !ncmdump_avail {
                    let _ = sender.send(TaskMessage::Log(format!(
                        "\n=== {} ===\n未找到 ncmdump, 无法自动转换 NCM.\n",
                        item.path.file_name().unwrap_or_default().to_string_lossy()
                    )));
                    let _ = sender.send(TaskMessage::InputStatus(
                        idx,
                        "跳过: 未找到 ncmdump".into(),
                        None,
                    ));
                    let _ = sender.send(TaskMessage::Progress(file_start + file_span));
                    continue;
                }
                let _ = sender.send(TaskMessage::InputStatus(idx, "转换 NCM...".into(), None));
                let _ = sender.send(TaskMessage::Status(format!(
                    "转换 NCM {}/{}: {}",
                    r,
                    total,
                    item.path.file_name().unwrap_or_default().to_string_lossy()
                )));
                let _ = sender.send(TaskMessage::Log(format!(
                    "\n=== {} ===\n正在自动转换 NCM...\n",
                    item.path.file_name().unwrap_or_default().to_string_lossy()
                )));
                let parent = item.path.parent().unwrap_or(Path::new(".")).to_path_buf();
                let _ = sender.send(TaskMessage::Progress(Self::progress_value(
                    ProgressScope {
                        start: file_start,
                        span: file_span,
                    },
                    0.05,
                )));
                match Self::convert_ncm_streaming(
                    ncmdump_path.as_ref().unwrap(),
                    &item.path,
                    &parent,
                    &sender,
                    running.clone(),
                    Some(ProgressScope {
                        start: file_start,
                        span: file_span * 0.20,
                    }),
                ) {
                    Ok(c) => {
                        item.process_path = Some(c.clone());
                        converted_this_item = true;
                        item.status = format!(
                            "已转换 {}",
                            c.extension()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_uppercase()
                        );
                        let _ = sender.send(TaskMessage::InputStatus(
                            idx,
                            item.status.clone(),
                            Some(c.to_string_lossy().to_string()),
                        ));
                        let _ = sender.send(TaskMessage::Log(format!(
                            "NCM 转换成功: {}\n",
                            c.file_name().unwrap_or_default().to_string_lossy()
                        )));
                        let _ = sender.send(TaskMessage::Progress(file_start + file_span * 0.20));
                    }
                    Err(e) => {
                        let _ =
                            sender.send(TaskMessage::InputStatus(idx, "NCM 转换失败".into(), None));
                        let _ = sender.send(TaskMessage::Log(format!("NCM 转换失败: {e}\n")));
                        fail += 1;
                        let _ = sender.send(TaskMessage::Progress(file_start + file_span));
                        continue;
                    }
                }
            }

            if !item.can_separate() {
                let _ = sender.send(TaskMessage::InputStatus(
                    idx,
                    "跳过: 无可处理音频".into(),
                    None,
                ));
                let _ = sender.send(TaskMessage::Log(format!(
                    "\n=== {} ===\n未找到同名 FLAC/MP3/WAV, 已跳过.\n",
                    item.path.file_name().unwrap_or_default().to_string_lossy()
                )));
                let _ = sender.send(TaskMessage::Progress(file_start + file_span));
                continue;
            }

            let _ = sender.send(TaskMessage::InputStatus(idx, "处理中".into(), None));
            let audio = item.process_path.as_ref().unwrap();
            let _ = sender.send(TaskMessage::Status(format!(
                "分离 {}/{}: {}",
                r,
                total,
                audio.file_name().unwrap_or_default().to_string_lossy()
            )));
            let _ = sender.send(TaskMessage::Log(format!(
                "\n=== {} ===\n使用音频: {}\n",
                item.path.file_name().unwrap_or_default().to_string_lossy(),
                audio.display()
            )));

            let cfg = find_separator(&separators, &sep_name).cloned();
            let cmd = if let Some(ref cfg) = cfg {
                let mut c = cfg.command.clone();
                for arg in &cfg.args_before {
                    c.push(
                        arg.replace("{model}", &model)
                            .replace("{output}", &output_dir.to_string_lossy().to_string()),
                    );
                }
                if cfg.stems_mode == "flag" && mode == "vocals" && !cfg.two_stem_flag.is_empty() {
                    c.push(cfg.two_stem_flag.clone());
                }
                if device != "auto" && !cfg.device_flag.is_empty() {
                    c.push(format!("{}", cfg.device_flag));
                    c.push(device.clone());
                }
                c.push(audio.to_string_lossy().to_string());
                c
            } else {
                // fallback Demucs
                let mut c = demucs_base.clone();
                let m = if mode == "six_stems" && model != "htdemucs_6s" {
                    "htdemucs_6s".to_string()
                } else {
                    model.clone()
                };
                c.extend_from_slice(&[
                    "-n".into(),
                    m,
                    "-o".into(),
                    output_dir.to_string_lossy().to_string(),
                ]);
                if mode == "vocals" {
                    c.push("--two-stems=vocals".into());
                }
                if device != "auto" {
                    c.extend_from_slice(&["-d".into(), device.clone()]);
                }
                c.push(audio.to_string_lossy().to_string());
                c
            };

            let mut env: std::collections::HashMap<String, String> = std::env::vars().collect();
            env.entry("PYTHONIOENCODING".into())
                .or_insert_with(|| "utf-8".into());
            env.entry("PYTHONUTF8".into()).or_insert_with(|| "1".into());
            if let Some(program) = cmd.first() {
                Self::add_python_runtime_paths(&mut env, program);
            }

            let demucs_start = if converted_this_item {
                file_start + file_span * 0.20
            } else {
                file_start
            };
            let demucs_span = if converted_this_item {
                file_span * 0.80
            } else {
                file_span
            };
            match Self::run_command_streaming(
                &cmd,
                Some(&env),
                &sender,
                running.clone(),
                Some(ProgressScope {
                    start: demucs_start,
                    span: demucs_span,
                }),
            ) {
                Ok(exit) => {
                    if exit.success() {
                        ok += 1;
                        let _ = sender.send(TaskMessage::InputStatus(idx, "已完成".into(), None));
                    } else {
                        fail += 1;
                        let _ = sender.send(TaskMessage::InputStatus(
                            idx,
                            format!("失败 {}", exit.code().unwrap_or(-1)),
                            None,
                        ));
                    }
                }
                Err(e) => {
                    fail += 1;
                    let _ =
                        sender.send(TaskMessage::InputStatus(idx, "未安装 Demucs".into(), None));
                    let _ = sender.send(TaskMessage::Log(format!("启动 Demucs 失败: {e}\n")));
                    break;
                }
            }
            let _ = sender.send(TaskMessage::Progress(file_start + file_span));
        }
        let _ = sender.send(TaskMessage::Status(format!(
            "分离完成 {}, 失败 {}",
            ok, fail
        )));
        let _ = sender.send(TaskMessage::Done);
    }

    fn run_ncm_convert_batch(
        items: Vec<AudioItem>,
        indices: Vec<usize>,
        ncmdump_path: PathBuf,
        sender: Sender<TaskMessage>,
        running: ArcBool,
    ) {
        let total = indices.len();
        let (mut ok, mut fail) = (0u32, 0u32);
        for (round, (idx, item)) in indices.into_iter().zip(items).enumerate() {
            if !*running.lock().unwrap() {
                break;
            }
            let r = round as u32 + 1;
            let file_start = round as f32 / total as f32;
            let file_span = 1.0 / total as f32;
            let _ = sender.send(TaskMessage::InputStatus(idx, "转换中...".into(), None));
            let _ = sender.send(TaskMessage::Status(format!(
                "转换 NCM {}/{}: {}",
                r,
                total,
                item.path.file_name().unwrap_or_default().to_string_lossy()
            )));
            let _ = sender.send(TaskMessage::Log(format!(
                "\n=== {} ===\n正在转换 NCM...\n",
                item.path.file_name().unwrap_or_default().to_string_lossy()
            )));
            let parent = item.path.parent().unwrap_or(Path::new(".")).to_path_buf();
            let _ = sender.send(TaskMessage::Progress(Self::progress_value(
                ProgressScope {
                    start: file_start,
                    span: file_span,
                },
                0.10,
            )));
            match Self::convert_ncm_streaming(
                &ncmdump_path,
                &item.path,
                &parent,
                &sender,
                running.clone(),
                Some(ProgressScope {
                    start: file_start,
                    span: file_span,
                }),
            ) {
                Ok(c) => {
                    ok += 1;
                    let ext = c
                        .extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_uppercase();
                    let _ = sender.send(TaskMessage::InputStatus(
                        idx,
                        format!("已转换 {}", ext),
                        Some(c.to_string_lossy().to_string()),
                    ));
                    let _ = sender.send(TaskMessage::Log(format!(
                        "转换成功: {}\n",
                        c.file_name().unwrap_or_default().to_string_lossy()
                    )));
                }
                Err(e) => {
                    fail += 1;
                    let _ = sender.send(TaskMessage::InputStatus(idx, "转换失败".into(), None));
                    let _ = sender.send(TaskMessage::Log(format!("转换失败: {e}\n")));
                }
            }
            let _ = sender.send(TaskMessage::Progress(file_start + file_span));
        }
        let _ = sender.send(TaskMessage::Status(format!(
            "转换完成 {}, 失败 {}",
            ok, fail
        )));
        let _ = sender.send(TaskMessage::Done);
    }

    // ── player ──

    fn play_audio(&mut self, path: &Path) {
        self.stop_playback();
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(_) => {
                self.status = "播放失败".into();
                return;
            }
        };
        match Decoder::new(file) {
            Ok(src) => {
                if let Some(ref h) = self.stream_handle {
                    if let Ok(sink) = Sink::try_new(h) {
                        sink.append(src);
                        self.sink = Some(sink);
                        self.now_playing = format!(
                            "正在播放: {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        self.paused = false;
                        self.status = "播放中".into();
                        return;
                    }
                }
                self.status = "播放失败".into();
            }
            Err(_) => {
                self.status = "播放失败".into();
            }
        }
    }

    fn stop_playback(&mut self) {
        if let Some(s) = self.sink.take() {
            s.stop();
        }
        self.now_playing = "未播放".into();
        self.paused = false;
    }

    fn pause_resume(&mut self) {
        if let Some(ref s) = self.sink {
            if self.paused {
                s.play();
                self.paused = false;
                self.status = "播放中".into();
            } else {
                s.pause();
                self.paused = true;
                self.status = "已暂停".into();
            }
        }
    }

    fn selected_ncm_indices(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.selected_items.iter().copied().collect();
        v.sort();
        v.retain(|&i| i < self.items.len() && self.items[i].kind == AudioKind::Ncm);
        v
    }

    fn selected_audio_indices(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.selected_items.iter().copied().collect();
        v.sort();
        v
    }
}

// ── egui app ──

impl eframe::App for StemStudio {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.pump_task_messages();
        let running = *self.task_running.lock().unwrap();

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.heading(APP_NAME);
                ui.separator();
                if ui
                    .add_enabled(!running, egui::Button::new("扫描"))
                    .clicked()
                {
                    self.scan_inputs();
                }
                if ui
                    .add_enabled(!running, egui::Button::new("分离选中"))
                    .clicked()
                {
                    self.separate_selected();
                }
                if ui
                    .add_enabled(!running, egui::Button::new("转换 NCM"))
                    .clicked()
                {
                    self.convert_ncm_selected();
                }
                if ui
                    .add_enabled(running, egui::Button::new("停止任务"))
                    .clicked()
                {
                    self.stop_task();
                }
                if ui.button("刷新输出").clicked() {
                    self.scan_stems();
                }
                if ui.button("打开输出目录").clicked() {
                    self.open_output_dir();
                }
                ui.separator();
                ui.label(
                    egui::RichText::new(if running { "任务运行中" } else { "空闲" }).color(
                        if running {
                            egui::Color32::from_rgb(250, 204, 21)
                        } else {
                            egui::Color32::from_rgb(74, 222, 128)
                        },
                    ),
                );
            });
        });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(self.progress)
                        .desired_width(f32::INFINITY)
                        .show_percentage(),
                );
                ui.label(&self.status);
            });
        });

        egui::TopBottomPanel::bottom("player").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("播放输入").clicked() {
                    self.play_selected_input();
                }
                if ui.button("播放音轨").clicked() {
                    self.play_selected_stem();
                }
                if ui
                    .button(if self.paused { "继续" } else { "暂停" })
                    .clicked()
                {
                    self.pause_resume();
                }
                if ui.button("停止播放").clicked() {
                    self.stop_playback();
                }
                ui.label(&self.now_playing);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("音频目录");
                let width = (ui.available_width() - 70.0).max(260.0);
                ui.add_sized(
                    egui::vec2(width, 22.0),
                    egui::TextEdit::singleline(&mut self.source_dir).desired_width(f32::INFINITY),
                );
                if ui
                    .add_enabled(!running, egui::Button::new("浏览"))
                    .clicked()
                {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.source_dir = dir.to_string_lossy().to_string();
                        self.save_settings();
                        self.scan_inputs();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("输出目录");
                let width = (ui.available_width() - 70.0).max(260.0);
                ui.add_sized(
                    egui::vec2(width, 22.0),
                    egui::TextEdit::singleline(&mut self.output_dir).desired_width(f32::INFINITY),
                );
                if ui
                    .add_enabled(!running, egui::Button::new("浏览"))
                    .clicked()
                {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.output_dir = dir.to_string_lossy().to_string();
                        self.save_settings();
                        self.scan_stems();
                    }
                }
            });

            ui.horizontal(|ui| {
                ui.label("工具");
                egui::ComboBox::from_id_salt("separator")
                    .selected_text(&self.separator)
                    .show_ui(ui, |ui| {
                        let names: Vec<String> =
                            self.separators.iter().map(|s| s.name.clone()).collect();
                        for t in &names {
                            if ui.selectable_label(self.separator == *t, t).clicked() {
                                self.separator = t.clone();
                                // reset mode/model to first of new separator
                                if let Some(cfg) = find_separator(&self.separators, t) {
                                    if let Some(m) = cfg.modes.first() {
                                        self.mode = m.clone();
                                    }
                                    if let Some(m) = cfg.models.first() {
                                        self.model = m.clone();
                                    }
                                }
                            }
                        }
                    });
                ui.label("分离模式");
                egui::ComboBox::from_id_salt("mode")
                    .selected_text(&self.mode)
                    .show_ui(ui, |ui| {
                        let modes: Vec<String> = find_separator(&self.separators, &self.separator)
                            .map(|c| c.modes.clone())
                            .unwrap_or_default();
                        for m in &modes {
                            if ui.selectable_label(self.mode == *m, m).clicked() {
                                self.mode = m.clone();
                            }
                        }
                    });
                ui.label("模型");
                egui::ComboBox::from_id_salt("model")
                    .selected_text(&self.model)
                    .show_ui(ui, |ui| {
                        let models: Vec<String> = find_separator(&self.separators, &self.separator)
                            .map(|c| c.models.clone())
                            .unwrap_or_default();
                        for m in &models {
                            if ui.selectable_label(self.model == *m, m).clicked() {
                                self.model = m.clone();
                            }
                        }
                    });
                ui.label("设备");
                egui::ComboBox::from_id_salt("device")
                    .selected_text(&self.device)
                    .show_ui(ui, |ui| {
                        for d in &["auto", "cpu", "cuda"] {
                            if ui.selectable_label(self.device == *d, *d).clicked() {
                                self.device = d.to_string();
                            }
                        }
                    });
            });

            let ready = self.items.iter().filter(|i| i.can_separate()).count();
            let ncm = self
                .items
                .iter()
                .filter(|i| i.kind == AudioKind::Ncm)
                .count();
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(format!("输入 {} 个", self.items.len())).strong());
                ui.label(format!("可分离 {ready} 个"));
                ui.label(format!("NCM {ncm} 个"));
                ui.label(format!("输出音轨 {} 个", self.stems.len()));
                ui.label(format!("已选 {} 个", self.selected_items.len()));
            });
            ui.label(
                egui::RichText::new(&self.tool_status)
                    .color(egui::Color32::GRAY)
                    .small(),
            );
            ui.separator();

            let avail = ui.available_size();
            let left_w = avail.x * 0.55;
            let table_h = (avail.y - 34.0).max(180.0);

            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_min_width(left_w);
                    ui.heading("输入音频");
                    let rh = 24.0;
                    TableBuilder::new(ui)
                        .striped(true)
                        .min_scrolled_height(table_h)
                        .max_scroll_height(table_h)
                        .auto_shrink([false, false])
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::initial(260.0))
                        .column(Column::initial(70.0))
                        .column(Column::initial(120.0))
                        .column(Column::remainder())
                        .header(rh, |mut h| {
                            h.col(|ui| {
                                ui.strong("文件");
                            });
                            h.col(|ui| {
                                ui.strong("类型");
                            });
                            h.col(|ui| {
                                ui.strong("状态");
                            });
                            h.col(|ui| {
                                ui.strong("路径");
                            });
                        })
                        .body(|body| {
                            body.rows(rh, self.items.len(), |mut row| {
                                let i = row.index();
                                let item = &self.items[i];
                                let sel = self.selected_items.contains(&i);
                                if sel {
                                    row.set_selected(true);
                                }
                                let ext = item
                                    .path
                                    .extension()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_uppercase();
                                let pt = item
                                    .process_path
                                    .as_ref()
                                    .map(|p| p.to_string_lossy().to_string())
                                    .unwrap_or_else(|| item.path.to_string_lossy().to_string());
                                let sc = status_color(&item.status);
                                row.col(|ui| {
                                    if ui
                                        .selectable_label(
                                            sel,
                                            item.path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy(),
                                        )
                                        .clicked()
                                    {
                                        if sel {
                                            self.selected_items.remove(&i);
                                        } else {
                                            self.selected_items.clear();
                                            self.selected_items.insert(i);
                                        }
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(&ext);
                                });
                                row.col(|ui| {
                                    ui.label(egui::RichText::new(&item.status).color(sc));
                                });
                                row.col(|ui| {
                                    ui.label(&pt);
                                });
                            });
                        });
                });

                ui.separator();

                ui.vertical(|ui| {
                    ui.set_min_width(avail.x - left_w);
                    ui.heading("输出音轨");
                    let rh = 23.0;
                    TableBuilder::new(ui)
                        .striped(true)
                        .min_scrolled_height(table_h)
                        .max_scroll_height(table_h)
                        .auto_shrink([false, false])
                        .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                        .column(Column::initial(180.0))
                        .column(Column::initial(80.0))
                        .column(Column::remainder())
                        .header(rh, |mut h| {
                            h.col(|ui| {
                                ui.strong("歌曲");
                            });
                            h.col(|ui| {
                                ui.strong("音轨");
                            });
                            h.col(|ui| {
                                ui.strong("路径");
                            });
                        })
                        .body(|body| {
                            body.rows(rh, self.stems.len(), |mut row| {
                                let i = row.index();
                                let stem = &self.stems[i];
                                let sel = self.selected_stem == Some(i);
                                row.set_selected(sel);
                                row.col(|ui| {
                                    if ui.selectable_label(sel, &stem.track_name).clicked() {
                                        self.selected_stem = Some(i);
                                    }
                                });
                                row.col(|ui| {
                                    ui.label(&stem.stem_name);
                                });
                                row.col(|ui| {
                                    ui.label(stem.path.to_string_lossy());
                                });
                            });
                        });
                });
            });
        });

        if running {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }
}

// ── actions ──

impl StemStudio {
    fn separate_selected(&mut self) {
        if *self.task_running.lock().unwrap() {
            self.status = "已有任务正在运行".into();
            return;
        }
        let indices = self.selected_audio_indices();
        if indices.is_empty() {
            self.status = "请先选择音频".into();
            return;
        }
        self.save_settings();
        let items: Vec<_> = indices
            .iter()
            .filter_map(|&i| self.items.get(i).cloned())
            .collect();
        let i2 = indices.clone();
        let od = PathBuf::from(&self.output_dir);
        let _ = std::fs::create_dir_all(&od);
        let na = self.ncmdump_available;
        let np = self.ncmdump_path.clone();
        let dc = self.demucs_command.clone();
        let sc = self.separators.clone();
        let m = self.model.clone();
        let mo = self.mode.clone();
        let d = self.device.clone();
        let sep = self.separator.clone();
        let running = self.task_running.clone();
        let (tx, rx) = mpsc::channel();
        self.task_receiver = Some(rx);
        *self.task_running.lock().unwrap() = true;
        self.progress = 0.0;
        thread::spawn(move || {
            Self::run_demucs_batch(items, i2, od, na, np, dc, sc, m, sep, mo, d, tx, running)
        });
    }

    fn convert_ncm_selected(&mut self) {
        if *self.task_running.lock().unwrap() {
            self.status = "已有任务正在运行".into();
            return;
        }
        let indices = self.selected_ncm_indices();
        if indices.is_empty() {
            self.status = "选中项中没有 NCM 文件".into();
            return;
        }
        self.save_settings();
        let items: Vec<_> = indices
            .iter()
            .filter_map(|&i| self.items.get(i).cloned())
            .collect();
        let i2 = indices.clone();
        let np = match self.ncmdump_path.clone() {
            Some(p) => p,
            None => {
                self.status = "未找到 ncmdump".into();
                return;
            }
        };
        let running = self.task_running.clone();
        let (tx, rx) = mpsc::channel();
        self.task_receiver = Some(rx);
        *self.task_running.lock().unwrap() = true;
        self.progress = 0.0;
        thread::spawn(move || Self::run_ncm_convert_batch(items, i2, np, tx, running));
    }

    fn stop_task(&mut self) {
        *self.task_running.lock().unwrap() = false;
        self.status = "已请求停止任务".into();
    }

    fn open_output_dir(&mut self) {
        let out = Path::new(&self.output_dir);
        let _ = std::fs::create_dir_all(out);
        let mut command = Command::new("explorer");
        command.arg(out);
        Self::hide_child_window(&mut command);
        let _ = command.spawn();
    }

    fn play_selected_input(&mut self) {
        let path = self
            .selected_items
            .iter()
            .next()
            .and_then(|&i| self.items.get(i))
            .and_then(|it| it.process_path.clone());
        match path {
            Some(p) => self.play_audio(&p),
            None => {
                self.status = "请先选择输入音频".into();
            }
        }
    }

    fn play_selected_stem(&mut self) {
        let path = self
            .selected_stem
            .and_then(|i| self.stems.get(i))
            .map(|s| s.path.clone());
        match path {
            Some(p) => self.play_audio(&p),
            None => {
                self.status = "请先选择输出音轨".into();
            }
        }
    }

    fn pump_task_messages(&mut self) {
        let mut done = false;
        let mut messages = Vec::new();
        if let Some(rx) = self.task_receiver.as_ref() {
            loop {
                match rx.try_recv() {
                    Ok(msg) => messages.push(msg),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        done = true;
                        break;
                    }
                }
            }
        }

        for msg in messages {
            match msg {
                TaskMessage::Log(_) => {}
                TaskMessage::Progress(p) => self.progress = p,
                TaskMessage::Status(s) => self.status = s,
                TaskMessage::InputStatus(idx, st, pp) => {
                    if let Some(item) = self.items.get_mut(idx) {
                        item.status = st;
                        if let Some(p) = pp {
                            item.process_path = Some(PathBuf::from(&p));
                        }
                    }
                }
                TaskMessage::Done => done = true,
            }
        }
        if done {
            *self.task_running.lock().unwrap() = false;
            self.task_receiver = None;

            // stamp metadata from source to stems
            let output = PathBuf::from(&self.output_dir);
            for item in &self.items {
                if let Some(ref src) = item.process_path {
                    let _ = yyw::stamp_metadata_for_source(src, &output);
                }
            }
            self.scan_stems();
        }
    }
}

fn main() {
    env_logger::init();

    // generate a simple teal icon (32x32)
    let icon = {
        let size = 32u32;
        let mut rgba = vec![0u8; (size * size * 4) as usize];
        let accent = egui::Color32::from_rgb(45, 212, 191);
        for y in 0..size {
            for x in 0..size {
                let i = ((y * size + x) * 4) as usize;
                // rounded rect shape
                let cx = x as f32 - size as f32 / 2.0;
                let cy = y as f32 - size as f32 / 2.0;
                let r = size as f32 / 2.0 - 2.0;
                let d = (cx * cx + cy * cy).sqrt();
                let alpha = if d < r - 3.0 {
                    255
                } else if d < r {
                    ((r - d) / 3.0 * 255.0) as u8
                } else {
                    0
                };
                rgba[i] = accent.r();
                rgba[i + 1] = accent.g();
                rgba[i + 2] = accent.b();
                rgba[i + 3] = alpha;
            }
        }
        egui::IconData {
            rgba,
            width: size,
            height: size,
        }
    };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(icon)
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([980.0, 640.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        APP_NAME,
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(egui::Visuals::dark());
            let mut style = (*cc.egui_ctx.style()).clone();
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(10.0, 5.0);
            cc.egui_ctx.set_style(style);

            // CJK font
            for path in &[
                r"C:\Windows\Fonts\msyh.ttc",
                r"C:\Windows\Fonts\msyh.ttf",
                r"C:\Windows\Fonts\simhei.ttf",
            ] {
                if let Ok(data) = std::fs::read(path) {
                    let mut fonts = egui::FontDefinitions::default();
                    fonts
                        .font_data
                        .insert("cjk".into(), Arc::new(egui::FontData::from_owned(data)));
                    fonts
                        .families
                        .entry(egui::FontFamily::Proportional)
                        .or_default()
                        .insert(0, "cjk".into());
                    fonts
                        .families
                        .entry(egui::FontFamily::Monospace)
                        .or_default()
                        .insert(0, "cjk".into());
                    cc.egui_ctx.set_fonts(fonts);
                    break;
                }
            }

            let mut app = StemStudio::new();
            app.scan_inputs();
            Ok(Box::new(app))
        }),
    );
}
