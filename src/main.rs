#![windows_subsystem = "windows"]

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Read};
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

#[derive(Clone)]
struct MediaInfo {
    title: String,
    path: PathBuf,
    rows: Vec<(String, String)>,
    raw: String,
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
    current_path: Option<PathBuf>,
    playback_duration: Option<Duration>,
    playback_offset: Duration,
    cover_texture: Option<egui::TextureHandle>,
    cover_status: String,
    lyrics_text: String,
    lyrics_status: String,
    media_info: Option<MediaInfo>,
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

fn load_png_icon(bytes: &[u8]) -> Option<egui::IconData> {
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image
        .resize(64, 64, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    Some(egui::IconData {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn load_color_image(path: &Path) -> Option<egui::ColorImage> {
    let image = image::ImageReader::open(path)
        .ok()?
        .decode()
        .ok()?
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(
        size,
        image.as_raw(),
    ))
}

fn format_time(duration: Duration) -> String {
    let secs = duration.as_secs();
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn valid_image_ext(path: &Path) -> bool {
    path.extension()
        .map(|ext| {
            matches!(
                ext.to_string_lossy().to_lowercase().as_str(),
                "png" | "jpg" | "jpeg"
            )
        })
        .unwrap_or(false)
}

fn compact_text(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 3 {
        return "...".to_string();
    }
    let head = (max_chars - 3) / 2;
    let tail = max_chars - 3 - head;
    let start: String = chars.iter().take(head).collect();
    let end: String = chars.iter().skip(chars.len() - tail).collect();
    format!("{start}...{end}")
}

fn compact_path(path: &Path, max_chars: usize) -> String {
    compact_text(&path.to_string_lossy(), max_chars)
}

fn compact_parent_path(path: &Path, max_chars: usize) -> String {
    path.parent()
        .map(|parent| compact_path(parent, max_chars))
        .unwrap_or_else(|| compact_path(path, max_chars))
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

        let mut app = Self {
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
            current_path: None,
            playback_duration: None,
            playback_offset: Duration::ZERO,
            cover_texture: None,
            cover_status: "未加载封面".to_string(),
            lyrics_text: "未加载歌词".to_string(),
            lyrics_status: "未加载歌词".to_string(),
            media_info: None,
        };
        app.scan_stems();
        app
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
        yyw::find_tool(name)
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
        if let Some(demucs) = Self::find_tool("demucs.exe") {
            return vec![demucs.to_string_lossy().to_string()];
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
            self.selected_stem = None;
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
        if self
            .selected_stem
            .map(|i| i >= self.stems.len())
            .unwrap_or(false)
        {
            self.selected_stem = None;
        }
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

    fn related_source_for(&self, path: &Path) -> PathBuf {
        if let Some(found) = self
            .items
            .iter()
            .filter_map(|item| item.process_path.as_ref())
            .find(|p| *p == path)
        {
            return found.clone();
        }

        let track_name = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string());
        if let Some(track_name) = track_name {
            if let Some(found) = self
                .items
                .iter()
                .filter_map(|item| item.process_path.as_ref())
                .find(|p| {
                    p.file_stem()
                        .map(|s| s.to_string_lossy() == track_name)
                        .unwrap_or(false)
                })
            {
                return found.clone();
            }

            let source_root = Path::new(&self.source_dir);
            if source_root.exists() {
                let mut candidates = Vec::new();
                for entry in WalkDir::new(source_root).into_iter().filter_map(|e| e.ok()) {
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let Some(ext) = entry.path().extension() else {
                        continue;
                    };
                    if !INPUT_EXTS.contains(&ext.to_string_lossy().to_lowercase().as_str()) {
                        continue;
                    }
                    if entry
                        .path()
                        .file_stem()
                        .map(|s| s.to_string_lossy() == track_name)
                        .unwrap_or(false)
                    {
                        candidates.push(entry.path().to_path_buf());
                    }
                }
                candidates.sort_by_key(|candidate| {
                    let ext = candidate
                        .extension()
                        .map(|ext| ext.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    if CONVERTED_EXTS.contains(&ext.as_str()) {
                        0
                    } else if ext == "ncm" {
                        2
                    } else {
                        1
                    }
                });
                if let Some(candidate) = candidates.into_iter().next() {
                    return candidate;
                }
            }
        }

        path.to_path_buf()
    }

    fn media_candidates_for(&self, path: &Path) -> Vec<PathBuf> {
        let mut candidates = vec![path.to_path_buf()];
        let source = self.related_source_for(path);
        if source != path {
            candidates.push(source);
        }
        let flac_sibling = path.with_extension("flac");
        if flac_sibling.exists() && flac_sibling != path {
            candidates.push(flac_sibling);
        }
        candidates.dedup();
        candidates
    }

    fn find_lyrics_file(&self, path: &Path) -> Option<PathBuf> {
        for base in self.media_candidates_for(path) {
            for ext in ["lrc", "txt"] {
                let candidate = base.with_extension(ext);
                if candidate.exists() {
                    return Some(candidate);
                }
            }
            if let Some(parent) = base.parent() {
                for name in ["lyrics.lrc", "lyrics.txt"] {
                    let candidate = parent.join(name);
                    if candidate.exists() {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    fn find_cover_file(&self, path: &Path) -> Option<PathBuf> {
        for base in self.media_candidates_for(path) {
            for ext in ["png", "jpg", "jpeg"] {
                let candidate = base.with_extension(ext);
                if candidate.exists() && valid_image_ext(&candidate) {
                    return Some(candidate);
                }
            }
            if let Some(parent) = base.parent() {
                for name in [
                    "cover.png",
                    "cover.jpg",
                    "cover.jpeg",
                    "folder.png",
                    "folder.jpg",
                    "folder.jpeg",
                ] {
                    let candidate = parent.join(name);
                    if candidate.exists() && valid_image_ext(&candidate) {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    fn extract_embedded_cover(&self, path: &Path) -> Option<PathBuf> {
        let ffmpeg = yyw::find_ffmpeg()?;
        for source in self.media_candidates_for(path) {
            if !source.exists() {
                continue;
            }
            let mut hasher = DefaultHasher::new();
            source.hash(&mut hasher);
            let out = std::env::temp_dir().join(format!("yyw_cover_{:x}.png", hasher.finish()));
            let mut command = Command::new(&ffmpeg);
            command
                .args([
                    "-y",
                    "-i",
                    &source.to_string_lossy(),
                    "-map",
                    "0:v:0?",
                    "-an",
                    "-frames:v",
                    "1",
                    "-update",
                    "1",
                    &out.to_string_lossy(),
                ])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            Self::hide_child_window(&mut command);
            if command.status().ok()?.success() && out.exists() {
                return Some(out);
            }
        }
        None
    }

    fn find_ffprobe() -> Option<PathBuf> {
        if let Some(ffmpeg) = yyw::find_ffmpeg() {
            let ffprobe = ffmpeg.with_file_name(if cfg!(windows) {
                "ffprobe.exe"
            } else {
                "ffprobe"
            });
            if ffprobe.exists() {
                return Some(ffprobe);
            }
        }
        Self::find_tool(if cfg!(windows) {
            "ffprobe.exe"
        } else {
            "ffprobe"
        })
    }

    fn probe_duration(path: &Path) -> Option<Duration> {
        let Some(ffprobe) = Self::find_ffprobe() else {
            return Self::probe_duration_with_ffmpeg(path);
        };
        let mut command = Command::new(ffprobe);
        command
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
                &path.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Self::hide_child_window(&mut command);
        let output = command.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let seconds = text.trim().parse::<f32>().ok()?;
        Some(Duration::from_secs_f32(seconds.max(0.0)))
    }

    fn probe_duration_with_ffmpeg(path: &Path) -> Option<Duration> {
        let ffmpeg = yyw::find_ffmpeg()?;
        let mut command = Command::new(ffmpeg);
        command
            .args(["-i", &path.to_string_lossy()])
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        Self::hide_child_window(&mut command);
        let output = command.output().ok()?;
        let text = String::from_utf8_lossy(&output.stderr);
        let duration = text
            .lines()
            .find_map(|line| line.split_once("Duration: ").map(|(_, rest)| rest))?
            .split(',')
            .next()?;
        Self::parse_duration_text(duration.trim())
    }

    fn parse_duration_text(text: &str) -> Option<Duration> {
        let mut parts = text.split(':');
        let hours = parts.next()?.parse::<f32>().ok()?;
        let minutes = parts.next()?.parse::<f32>().ok()?;
        let seconds = parts.next()?.parse::<f32>().ok()?;
        Some(Duration::from_secs_f32(
            (hours * 3600.0 + minutes * 60.0 + seconds).max(0.0),
        ))
    }

    fn extract_embedded_lyrics(&self, path: &Path) -> Option<String> {
        let ffprobe = Self::find_ffprobe()?;
        let source = self.related_source_for(path);
        let mut command = Command::new(ffprobe);
        command
            .args([
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_entries",
                "format_tags:stream_tags",
                &source.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Self::hide_child_window(&mut command);
        let output = command.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        Self::lyrics_from_probe_value(&value)
    }

    fn lyrics_from_probe_value(value: &serde_json::Value) -> Option<String> {
        fn find_in_tags(tags: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
            for (key, value) in tags {
                let key = key.to_lowercase();
                if !key.contains("lyric") {
                    continue;
                }
                let Some(text) = value.as_str() else {
                    continue;
                };
                let text = text.trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
            None
        }

        value
            .get("format")
            .and_then(|format| format.get("tags"))
            .and_then(|tags| tags.as_object())
            .and_then(find_in_tags)
            .or_else(|| {
                value
                    .get("streams")
                    .and_then(|streams| streams.as_array())
                    .and_then(|streams| {
                        streams.iter().find_map(|stream| {
                            stream
                                .get("tags")
                                .and_then(|tags| tags.as_object())
                                .and_then(find_in_tags)
                        })
                    })
            })
    }

    fn format_lyric_timestamp(ms: u64) -> String {
        let total_cs = ms / 10;
        let minutes = total_cs / 6000;
        let seconds = (total_cs / 100) % 60;
        let centiseconds = total_cs % 100;
        format!("[{minutes:02}:{seconds:02}.{centiseconds:02}]")
    }

    fn parse_netease_json_lyric_line(line: &str) -> Option<String> {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let text = value
            .get("c")
            .and_then(|chunks| chunks.as_array())
            .map(|chunks| {
                chunks
                    .iter()
                    .filter_map(|chunk| chunk.get("tx").and_then(|tx| tx.as_str()))
                    .collect::<String>()
            })?;
        let text = text.trim();
        if text.is_empty() {
            return None;
        }

        if let Some(ms) = value.get("t").and_then(|t| t.as_u64()) {
            Some(format!("{}{}", Self::format_lyric_timestamp(ms), text))
        } else {
            Some(text.to_string())
        }
    }

    fn normalize_lyrics(raw: &str) -> String {
        let lines: Vec<String> = raw
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    return None;
                }
                if trimmed.starts_with('{') && trimmed.ends_with('}') {
                    return Self::parse_netease_json_lyric_line(trimmed);
                }
                Some(trimmed.to_string())
            })
            .collect();

        if lines.is_empty() {
            "歌词为空".into()
        } else {
            lines.join("\n")
        }
    }

    fn file_size_text(bytes: u64) -> String {
        let units = ["B", "KB", "MB", "GB"];
        let mut value = bytes as f64;
        let mut unit = 0usize;
        while value >= 1024.0 && unit + 1 < units.len() {
            value /= 1024.0;
            unit += 1;
        }
        if unit == 0 {
            format!("{} {}", bytes, units[unit])
        } else {
            format!("{value:.2} {}", units[unit])
        }
    }

    fn bitrate_text(bits_per_second: u64) -> String {
        if bits_per_second >= 1_000_000 {
            format!("{:.2} Mbps", bits_per_second as f64 / 1_000_000.0)
        } else {
            format!("{:.0} kbps", bits_per_second as f64 / 1000.0)
        }
    }

    fn duration_detail_text(duration: Duration) -> String {
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            secs / 3600,
            (secs % 3600) / 60,
            secs % 60,
            millis
        )
    }

    fn media_info_for(path: &Path) -> MediaInfo {
        if let Some(info) = Self::media_info_from_ffprobe(path) {
            return info;
        }
        Self::media_info_from_ffmpeg(path)
    }

    fn media_info_from_ffprobe(path: &Path) -> Option<MediaInfo> {
        let ffprobe = Self::find_ffprobe()?;
        let mut command = Command::new(ffprobe);
        command
            .args([
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
                &path.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Self::hide_child_window(&mut command);
        let output = command.output().ok()?;
        if !output.status.success() {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
        let mut rows = Vec::new();
        if let Ok(meta) = std::fs::metadata(path) {
            rows.push(("文件大小".into(), Self::file_size_text(meta.len())));
        }
        if let Some(format) = value.get("format") {
            if let Some(name) = format.get("format_long_name").and_then(|v| v.as_str()) {
                rows.push(("格式".into(), name.to_string()));
            } else if let Some(name) = format.get("format_name").and_then(|v| v.as_str()) {
                rows.push(("格式".into(), name.to_string()));
            }
            if let Some(duration) = format
                .get("duration")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f32>().ok())
            {
                rows.push((
                    "时长".into(),
                    Self::duration_detail_text(Duration::from_secs_f32(duration.max(0.0))),
                ));
            }
            if let Some(bit_rate) = format
                .get("bit_rate")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<u64>().ok())
            {
                rows.push(("总体码率".into(), Self::bitrate_text(bit_rate)));
            }
        }
        if let Some(streams) = value.get("streams").and_then(|v| v.as_array()) {
            for stream in streams {
                let codec_type = stream
                    .get("codec_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("stream");
                let label = if codec_type == "audio" {
                    "音频流"
                } else if codec_type == "video" {
                    "视频/封面流"
                } else {
                    "数据流"
                };
                let mut details = Vec::new();
                if let Some(codec) = stream.get("codec_name").and_then(|v| v.as_str()) {
                    details.push(codec.to_string());
                }
                if let Some(rate) = stream.get("sample_rate").and_then(|v| v.as_str()) {
                    details.push(format!("{rate} Hz"));
                }
                if let Some(channels) = stream.get("channels").and_then(|v| v.as_u64()) {
                    details.push(format!("{channels} 声道"));
                }
                if let Some(bit_rate) = stream
                    .get("bit_rate")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    details.push(Self::bitrate_text(bit_rate));
                }
                if let (Some(w), Some(h)) = (
                    stream.get("width").and_then(|v| v.as_u64()),
                    stream.get("height").and_then(|v| v.as_u64()),
                ) {
                    details.push(format!("{w}x{h}"));
                }
                if !details.is_empty() {
                    rows.push((label.into(), details.join(" / ")));
                }
            }
        }
        if rows.is_empty() {
            return None;
        }
        Some(MediaInfo {
            title: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            path: path.to_path_buf(),
            rows,
            raw: String::from_utf8_lossy(&output.stdout).to_string(),
        })
    }

    fn media_info_from_ffmpeg(path: &Path) -> MediaInfo {
        let mut rows = Vec::new();
        if let Ok(meta) = std::fs::metadata(path) {
            rows.push(("文件大小".into(), Self::file_size_text(meta.len())));
        }
        let raw = yyw::find_ffmpeg()
            .and_then(|ffmpeg| {
                let mut command = Command::new(ffmpeg);
                command
                    .args(["-i", &path.to_string_lossy()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped());
                Self::hide_child_window(&mut command);
                command.output().ok()
            })
            .map(|output| String::from_utf8_lossy(&output.stderr).to_string())
            .unwrap_or_else(|| "未找到 ffmpeg，无法读取媒体流信息。".into());

        if let Some(duration) = raw
            .lines()
            .find_map(|line| line.split_once("Duration: ").map(|(_, rest)| rest))
            .and_then(|rest| rest.split(',').next())
            .and_then(|text| Self::parse_duration_text(text.trim()))
        {
            rows.push(("时长".into(), Self::duration_detail_text(duration)));
        }
        if let Some(bit_rate) = raw
            .lines()
            .find_map(|line| line.split_once("bitrate: ").map(|(_, rest)| rest))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|value| value.parse::<u64>().ok())
        {
            rows.push(("总体码率".into(), Self::bitrate_text(bit_rate * 1000)));
        }
        for line in raw.lines().filter(|line| line.contains("Stream #")) {
            if let Some((_, details)) = line.split_once(": Audio: ") {
                rows.push(("音频流".into(), details.trim().to_string()));
            } else if let Some((_, details)) = line.split_once(": Video: ") {
                rows.push(("视频/封面流".into(), details.trim().to_string()));
            }
        }
        if rows.is_empty() {
            rows.push(("状态".into(), "无法读取媒体信息".into()));
        }

        MediaInfo {
            title: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            path: path.to_path_buf(),
            rows,
            raw,
        }
    }

    fn open_media_location(path: &Path) {
        let mut command = Command::new("explorer");
        command.arg("/select,").arg(path);
        Self::hide_child_window(&mut command);
        let _ = command.spawn();
    }

    fn media_context_menu(
        &mut self,
        response: &egui::Response,
        ctx: &egui::Context,
        path: PathBuf,
    ) {
        response.context_menu(|ui| {
            if ui.button("播放").clicked() {
                self.play_audio(ctx, &path);
                ui.close_menu();
            }
            if ui.button("查看详细信息").clicked() {
                self.media_info = Some(Self::media_info_for(&path));
                ui.close_menu();
            }
            if ui.button("打开所在目录").clicked() {
                Self::open_media_location(&path);
                ui.close_menu();
            }
        });
    }

    fn load_media_sidecar(&mut self, ctx: &egui::Context, path: &Path) {
        self.cover_texture = None;
        self.cover_status = "未找到封面".into();
        self.lyrics_text = "未找到歌词文件".into();
        self.lyrics_status = "未找到歌词".into();

        if let Some(lyrics) = self.find_lyrics_file(path) {
            match std::fs::read_to_string(&lyrics) {
                Ok(text) => {
                    self.lyrics_text = Self::normalize_lyrics(&text);
                    self.lyrics_status = format!(
                        "歌词: {}",
                        lyrics.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
                Err(_) => {
                    self.lyrics_status = "歌词读取失败".into();
                    self.lyrics_text = "歌词读取失败".into();
                }
            }
        }
        if self.lyrics_status == "未找到歌词" {
            if let Some(text) = self.extract_embedded_lyrics(path) {
                self.lyrics_text = Self::normalize_lyrics(&text);
                self.lyrics_status = "歌词: 音频内嵌".into();
            }
        }

        let cover = self
            .find_cover_file(path)
            .or_else(|| self.extract_embedded_cover(path));
        if let Some(cover) = cover {
            if let Some(image) = load_color_image(&cover) {
                self.cover_texture = Some(ctx.load_texture("cover-art", image, Default::default()));
                self.cover_status = format!(
                    "封面: {}",
                    cover.file_name().unwrap_or_default().to_string_lossy()
                );
            } else {
                self.cover_status = "封面读取失败".into();
            }
        }
    }

    fn playback_position(&self) -> Duration {
        let pos = self.playback_offset
            + self
                .sink
                .as_ref()
                .map(|sink| sink.get_pos())
                .unwrap_or_default();
        if let Some(duration) = self.playback_duration {
            pos.min(duration)
        } else {
            pos
        }
    }

    fn seek_playback(&mut self, seconds: f32) {
        let Some(path) = self.current_path.clone() else {
            return;
        };
        let target = Duration::from_secs_f32(seconds.max(0.0));

        if let Some(sink) = &self.sink {
            if sink.try_seek(target).is_ok() {
                self.playback_offset = Duration::ZERO;
                return;
            }
        }

        let Ok(file) = File::open(&path) else {
            self.status = "跳转失败：无法打开音频".into();
            return;
        };
        let Ok(src) = Decoder::new(BufReader::new(file)) else {
            self.status = "跳转失败：无法解码音频".into();
            return;
        };
        let Some(ref h) = self.stream_handle else {
            self.status = "跳转失败：没有音频输出设备".into();
            return;
        };
        let Ok(sink) = Sink::try_new(h) else {
            self.status = "跳转失败：无法创建播放器".into();
            return;
        };

        let was_paused = self.paused;
        if let Some(old) = self.sink.take() {
            old.stop();
        }
        sink.append(src.skip_duration(target));
        if was_paused {
            sink.pause();
        }
        self.sink = Some(sink);
        self.playback_offset = target;
        self.paused = was_paused;
    }

    fn play_audio(&mut self, ctx: &egui::Context, path: &Path) {
        self.stop_playback();
        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => {
                self.status = "播放失败".into();
                return;
            }
        };
        match Decoder::new(BufReader::new(file)) {
            Ok(src) => {
                let duration = src.total_duration().or_else(|| Self::probe_duration(path));
                if let Some(ref h) = self.stream_handle {
                    if let Ok(sink) = Sink::try_new(h) {
                        sink.append(src);
                        self.sink = Some(sink);
                        self.current_path = Some(path.to_path_buf());
                        self.playback_duration = duration;
                        self.playback_offset = Duration::ZERO;
                        self.now_playing = format!(
                            "正在播放: {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        self.paused = false;
                        self.status = "播放中".into();
                        self.load_media_sidecar(ctx, path);
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
        self.current_path = None;
        self.playback_duration = None;
        self.playback_offset = Duration::ZERO;
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
                if let Some(texture) = &self.cover_texture {
                    ui.image((texture.id(), egui::vec2(76.0, 76.0)));
                } else {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(76.0, 76.0), egui::Sense::hover());
                    ui.painter().rect_filled(
                        rect,
                        egui::CornerRadius::same(4),
                        egui::Color32::from_gray(36),
                    );
                    ui.painter().text(
                        rect.center(),
                        egui::Align2::CENTER_CENTER,
                        "封面",
                        egui::FontId::proportional(13.0),
                        egui::Color32::GRAY,
                    );
                }

                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        if ui.button("播放输入").clicked() {
                            self.play_selected_input(ctx);
                        }
                        if ui.button("播放音轨").clicked() {
                            self.play_selected_stem(ctx);
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
                    });

                    ui.label(&self.now_playing);
                    let pos = self.playback_position();
                    let dur = self.playback_duration.unwrap_or_default();
                    ui.horizontal(|ui| {
                        ui.label(format_time(pos));
                        let mut seconds = pos.as_secs_f32();
                        let max = dur.as_secs_f32().max(1.0);
                        let response = ui.add(
                            egui::Slider::new(&mut seconds, 0.0..=max)
                                .show_value(false)
                                .text("播放进度"),
                        );
                        if response.changed() {
                            self.seek_playback(seconds);
                        }
                        ui.label(format_time(dur));
                    });
                    ui.label(
                        egui::RichText::new(format!(
                            "{} | {}",
                            self.cover_status, self.lyrics_status
                        ))
                        .small()
                        .color(egui::Color32::GRAY),
                    );
                });

                ui.separator();
                ui.vertical(|ui| {
                    ui.label("歌词");
                    egui::ScrollArea::vertical()
                        .max_height(76.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(&self.lyrics_text).small());
                        });
                });
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
            let gap_w = 10.0;
            let left_w = ((avail.x - gap_w) * 0.56).clamp(360.0, (avail.x - gap_w) * 0.68);
            let right_w = (avail.x - left_w - gap_w).max(240.0);
            let table_h = avail.y.max(220.0);

            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(left_w, table_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(left_w);
                        ui.heading("输入音频");
                        let rh = 24.0;
                        TableBuilder::new(ui)
                            .striped(true)
                            .min_scrolled_height(table_h)
                            .max_scroll_height(table_h)
                            .auto_shrink([false, false])
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(Column::remainder())
                            .column(Column::initial(56.0))
                            .column(Column::initial(104.0))
                            .column(Column::initial(128.0))
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
                                    let item_path = self.items[i].path.clone();
                                    let item_status = self.items[i].status.clone();
                                    let item_process_path = self.items[i].process_path.clone();
                                    let sel = self.selected_items.contains(&i);
                                    if sel {
                                        row.set_selected(true);
                                    }
                                    let ext = item_path
                                        .extension()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_uppercase();
                                    let pt = item_process_path
                                        .as_ref()
                                        .map(|p| p.to_string_lossy().to_string())
                                        .unwrap_or_else(|| item_path.to_string_lossy().to_string());
                                    let compact_pt = compact_parent_path(Path::new(&pt), 28);
                                    let sc = status_color(&item_status);
                                    let media_path = item_process_path
                                        .clone()
                                        .unwrap_or_else(|| item_path.clone());
                                    row.col(|ui| {
                                        let response = ui.selectable_label(
                                            sel,
                                            item_path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy(),
                                        );
                                        if response.clicked() {
                                            if sel {
                                                self.selected_items.remove(&i);
                                            } else {
                                                self.selected_items.clear();
                                                self.selected_items.insert(i);
                                            }
                                        }
                                        self.media_context_menu(&response, ctx, media_path.clone());
                                    });
                                    row.col(|ui| {
                                        ui.label(&ext);
                                    });
                                    row.col(|ui| {
                                        ui.label(egui::RichText::new(&item_status).color(sc));
                                    });
                                    row.col(|ui| {
                                        ui.label(&compact_pt).on_hover_text(&pt);
                                    });
                                });
                            });
                    },
                );

                ui.separator();

                ui.allocate_ui_with_layout(
                    egui::vec2(right_w, table_h),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_width(right_w);
                        ui.horizontal(|ui| {
                            ui.heading(format!("输出音轨 ({})", self.stems.len()));
                            if ui.small_button("刷新").clicked() {
                                self.scan_stems();
                            }
                        });
                        if self.stems.is_empty() {
                            ui.label(
                                egui::RichText::new("当前输出目录没有可播放的分轨")
                                    .small()
                                    .color(egui::Color32::GRAY),
                            );
                        }
                        ui.add_space(2.0);
                        egui::ScrollArea::vertical()
                            .id_salt("stems_scroll")
                            .max_height(table_h)
                            .auto_shrink([false, false])
                            .show_rows(ui, 72.0, self.stems.len(), |ui, range| {
                                for i in range {
                                    let track_name = self.stems[i].track_name.clone();
                                    let stem_name = self.stems[i].stem_name.clone();
                                    let path = self.stems[i].path.clone();
                                    let full_path_text = path.to_string_lossy().to_string();
                                    let sel = self.selected_stem == Some(i);
                                    let card_w = ui.available_width().max(220.0);
                                    let card_h = 64.0;
                                    let (rect, response) = ui.allocate_exact_size(
                                        egui::vec2(card_w, card_h),
                                        egui::Sense::click(),
                                    );
                                    let hovered = response.hovered();
                                    let fill = if sel {
                                        egui::Color32::from_rgb(54, 67, 84)
                                    } else if hovered {
                                        egui::Color32::from_rgb(48, 52, 59)
                                    } else {
                                        ui.visuals().widgets.noninteractive.bg_fill
                                    };
                                    let stroke = if sel {
                                        egui::Stroke::new(1.0, ui.visuals().selection.stroke.color)
                                    } else {
                                        ui.visuals().widgets.noninteractive.bg_stroke
                                    };
                                    ui.painter().rect(
                                        rect.shrink(2.0),
                                        egui::CornerRadius::same(6),
                                        fill,
                                        stroke,
                                        egui::StrokeKind::Outside,
                                    );
                                    #[allow(deprecated)]
                                    {
                                        ui.allocate_ui_at_rect(rect.shrink(10.0), |ui| {
                                            ui.set_width((card_w - 20.0).max(160.0));
                                            ui.label(
                                                egui::RichText::new(track_name)
                                                    .strong()
                                                    .color(ui.visuals().text_color()),
                                            );
                                            ui.add_space(4.0);
                                            ui.horizontal(|ui| {
                                                ui.label(
                                                    egui::RichText::new("音轨")
                                                        .small()
                                                        .color(egui::Color32::GRAY),
                                                );
                                                ui.label(
                                                    egui::RichText::new(stem_name).small().color(
                                                        egui::Color32::from_rgb(126, 200, 255),
                                                    ),
                                                );
                                            });
                                        });
                                    }
                                    let response = response.on_hover_text(full_path_text);
                                    if response.clicked() {
                                        self.selected_stem = Some(i);
                                    }
                                    if response.double_clicked() {
                                        self.selected_stem = Some(i);
                                        self.play_audio(ctx, &path);
                                    }
                                    self.media_context_menu(&response, ctx, path);
                                }
                            });
                    },
                );
            });
        });

        if let Some(info) = self.media_info.clone() {
            let mut open = true;
            egui::Window::new("媒体详细信息")
                .open(&mut open)
                .resizable(true)
                .default_width(560.0)
                .default_height(420.0)
                .show(ctx, |ui| {
                    ui.heading(&info.title);
                    ui.label(
                        egui::RichText::new(info.path.to_string_lossy())
                            .small()
                            .color(egui::Color32::GRAY),
                    );
                    ui.separator();
                    egui::Grid::new("media_info_grid")
                        .num_columns(2)
                        .spacing([18.0, 8.0])
                        .striped(true)
                        .show(ui, |ui| {
                            for (key, value) in &info.rows {
                                ui.strong(key);
                                ui.label(value);
                                ui.end_row();
                            }
                        });
                    ui.separator();
                    egui::CollapsingHeader::new("原始输出")
                        .default_open(false)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(160.0)
                                .show(ui, |ui| {
                                    ui.label(egui::RichText::new(&info.raw).monospace().small());
                                });
                        });
                    ui.horizontal(|ui| {
                        if ui.button("打开所在目录").clicked() {
                            Self::open_media_location(&info.path);
                        }
                        if ui.button("关闭").clicked() {
                            self.media_info = None;
                        }
                    });
                });
            if !open {
                self.media_info = None;
            }
        }

        let playback_active = self
            .sink
            .as_ref()
            .map(|sink| !sink.empty())
            .unwrap_or(false);
        if running || playback_active {
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

    fn play_selected_input(&mut self, ctx: &egui::Context) {
        let path = self
            .selected_items
            .iter()
            .next()
            .and_then(|&i| self.items.get(i))
            .and_then(|it| it.process_path.clone());
        match path {
            Some(p) => self.play_audio(ctx, &p),
            None => {
                self.status = "请先选择输入音频".into();
            }
        }
    }

    fn play_selected_stem(&mut self, ctx: &egui::Context) {
        if self.stems.is_empty() {
            self.scan_stems();
        }
        let path = self
            .selected_stem
            .and_then(|i| self.stems.get(i))
            .map(|s| s.path.clone());
        match path {
            Some(p) => self.play_audio(ctx, &p),
            None => {
                if self.stems.is_empty() {
                    self.status = "未找到输出音轨，请先分离或检查输出目录".into();
                } else {
                    self.status = format!("请先在右侧选择输出音轨；已找到 {} 个", self.stems.len());
                }
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

            // run metadata stamping in background (non-blocking)
            let output = PathBuf::from(&self.output_dir);
            let items: Vec<_> = self
                .items
                .iter()
                .filter_map(|it| it.process_path.clone())
                .collect();
            std::thread::spawn(move || {
                for src in &items {
                    let _ = yyw::stamp_metadata_for_source(src, &output);
                }
            });
            self.scan_stems();
        }
    }
}

fn main() {
    env_logger::init();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([980.0, 640.0]);
    if let Some(icon) = load_png_icon(include_bytes!("../radian.png")) {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
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
