use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
use walkdir::WalkDir;

pub const SETTINGS_FILE: &str = "stem_studio_settings.json";
pub const DEFAULT_SOURCE: &str = r"G:\CloudMusic\VipSongsDownload";
pub const DEFAULT_OUTPUT: &str = "stems_output";

pub const CONVERTED_EXTS: &[&str] = &[
    "flac", "mp3", "wav", "m4a", "aac", "ogg", "wma", "aiff", "aif",
];
pub const INPUT_EXTS: &[&str] = &[
    "flac", "mp3", "wav", "m4a", "aac", "ogg", "wma", "aiff", "aif", "ncm",
];
pub const STEM_EXTS: &[&str] = &["wav", "mp3", "flac", "m4a", "aac", "ogg", "wma"];

pub type ArcBool = Arc<Mutex<bool>>;

#[derive(Debug, Clone, Deserialize)]
pub struct SeparatorConfig {
    pub name: String,
    pub command: Vec<String>,
    pub models: Vec<String>,
    pub modes: Vec<String>,
    pub args_before: Vec<String>,
    pub two_stem_flag: String,
    pub device_flag: String,
    #[serde(default)]
    pub stems_mode: String,
}

pub fn load_separators() -> Vec<SeparatorConfig> {
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

pub fn find_separator<'a>(
    configs: &'a [SeparatorConfig],
    name: &str,
) -> Option<&'a SeparatorConfig> {
    configs.iter().find(|c| c.name == name)
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Settings {
    pub source: String,
    pub output: String,
    pub model: String,
    pub mode: String,
    pub device: String,
    pub separator: String,
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

impl Settings {
    pub fn load() -> Self {
        let path = Path::new(SETTINGS_FILE);
        if let Ok(data) = std::fs::read_to_string(path) {
            if let Ok(s) = serde_json::from_str(&data) {
                return s;
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let data = serde_json::to_string_pretty(self).unwrap_or_default();
        let _ = std::fs::write(SETTINGS_FILE, data);
    }
}

#[derive(Clone, PartialEq)]
pub enum AudioKind {
    Normal,
    Ncm,
}

#[derive(Clone)]
pub struct AudioItem {
    pub path: PathBuf,
    pub status: String,
    pub process_path: Option<PathBuf>,
    pub kind: AudioKind,
}

impl AudioItem {
    pub fn can_separate(&self) -> bool {
        self.process_path.is_some()
    }
}

#[derive(Clone)]
pub struct StemItem {
    pub path: PathBuf,
    pub track_name: String,
    pub stem_name: String,
}

pub enum TaskMessage {
    Log(String),
    Progress(f32),
    Status(String),
    InputStatus(usize, String, Option<String>),
    Done,
}

fn push_tool_roots(roots: &mut Vec<PathBuf>, root: PathBuf) {
    for dir in root.ancestors().take(4) {
        let dir = dir.to_path_buf();
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    }
}

pub fn find_tool(name: &str) -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push_tool_roots(&mut roots, dir.to_path_buf());
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        push_tool_roots(&mut roots, cwd);
    }

    for root in roots {
        for candidate in [root.join("tools").join(name), root.join(name)] {
            if candidate.exists() {
                return Some(candidate);
            }
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

pub fn find_demucs() -> Vec<String> {
    if let Ok(cwd) = std::env::current_dir() {
        let local = cwd.join("tools").join("demucs.exe");
        if local.exists() {
            return vec![local.to_string_lossy().to_string()];
        }
        let portable = cwd.join("runtime").join("python").join("python.exe");
        if portable.exists() {
            return vec![
                portable.to_string_lossy().to_string(),
                "-m".into(),
                "demucs".into(),
            ];
        }
    }
    if find_tool("demucs.exe").is_some() {
        return vec!["demucs".to_string()];
    }
    let conda = PathBuf::from(r"D:\conda\python.exe");
    if conda.exists() {
        return vec![
            conda.to_string_lossy().to_string(),
            "-m".into(),
            "demucs".into(),
        ];
    }
    vec!["python".to_string(), "-m".to_string(), "demucs".to_string()]
}

pub fn scan_inputs(source_dir: &str, items: &mut Vec<AudioItem>) -> usize {
    items.clear();
    let source = Path::new(source_dir);
    if !source.exists() {
        return 0;
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
    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    for path in files {
        let is_ncm = path.extension().map(|e| e == "ncm").unwrap_or(false);
        let item = if is_ncm {
            make_ncm_item(&path, source)
        } else {
            AudioItem {
                path: path.clone(),
                status: "待处理".into(),
                process_path: Some(path.clone()),
                kind: AudioKind::Normal,
            }
        };
        items.push(item);
    }
    items.len()
}

fn make_ncm_item(ncm_path: &Path, source_root: &Path) -> AudioItem {
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
            .then_with(|| a.extension().cmp(&b.extension()))
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

pub fn convert_ncm_sync(
    ncmdump: &Path,
    ncm_path: &Path,
    out_dir: &Path,
) -> Result<PathBuf, String> {
    let out = Command::new(ncmdump)
        .args([
            "-o",
            &out_dir.to_string_lossy(),
            &ncm_path.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("ncmdump 启动失败: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "ncmdump 返回错误: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let stem = ncm_path.file_stem().unwrap_or_default().to_string_lossy();
    for ext in &["flac", "mp3"] {
        let c = out_dir.join(format!("{}.{}", stem, ext));
        if c.exists() {
            return Ok(c);
        }
    }
    Err("ncmdump 未生成输出文件".into())
}

pub fn build_command(
    cfg: &SeparatorConfig,
    demucs_base: &[String],
    output_dir: &Path,
    audio: &Path,
    model: &str,
    mode: &str,
    device: &str,
) -> Vec<String> {
    let mut c = cfg.command.clone();
    for arg in &cfg.args_before {
        c.push(
            arg.replace("{model}", model)
                .replace("{output}", &output_dir.to_string_lossy().to_string()),
        );
    }
    if cfg.stems_mode == "flag" && mode == "vocals" && !cfg.two_stem_flag.is_empty() {
        c.push(cfg.two_stem_flag.clone());
    }
    if device != "auto" && !cfg.device_flag.is_empty() {
        c.push(cfg.device_flag.clone());
        c.push(device.to_string());
    }
    c.push(audio.to_string_lossy().to_string());

    // Fallback: if command is just demucs wrapper, use legacy builder
    if c.is_empty() && !demucs_base.is_empty() {
        let mut fc = demucs_base.to_vec();
        let m = if mode == "six_stems" && model != "htdemucs_6s" {
            "htdemucs_6s".to_string()
        } else {
            model.to_string()
        };
        fc.extend_from_slice(&[
            "-n".into(),
            m,
            "-o".into(),
            output_dir.to_string_lossy().to_string(),
        ]);
        if mode == "vocals" {
            fc.push("--two-stems=vocals".into());
        }
        if device != "auto" {
            fc.extend_from_slice(&["-d".into(), device.to_string()]);
        }
        fc.push(audio.to_string_lossy().to_string());
        return fc;
    }
    c
}

pub fn run_separation(
    ncmdump_avail: bool,
    ncmdump_path: &Option<PathBuf>,
    items: &[AudioItem],
    output_dir: &Path,
    separators: &[SeparatorConfig],
    separator_name: &str,
    model: &str,
    mode: &str,
    device: &str,
    demucs_base: &[String],
    sender: Sender<TaskMessage>,
    running: ArcBool,
) {
    let total = items.len();
    let (mut ok, mut fail) = (0u32, 0u32);

    for (idx, item) in items.iter().enumerate() {
        if !*running.lock().unwrap() {
            break;
        }
        let r = idx as u32 + 1;
        let mut item = item.clone();

        if item.kind == AudioKind::Ncm && !item.can_separate() {
            if !ncmdump_avail {
                let _ = sender.send(TaskMessage::Log(format!(
                    "\n=== {} ===\n未找到 ncmdump, 无法自动转换.\n",
                    item.path.file_name().unwrap_or_default().to_string_lossy()
                )));
                let _ = sender.send(TaskMessage::Progress(r as f32 / total as f32));
                continue;
            }
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
            match convert_ncm_sync(ncmdump_path.as_ref().unwrap(), &item.path, &parent) {
                Ok(c) => {
                    item.process_path = Some(c.clone());
                    let _ =
                        sender.send(TaskMessage::Log(format!("NCM 转换成功: {}\n", c.display())));
                }
                Err(e) => {
                    let _ = sender.send(TaskMessage::Log(format!("NCM 转换失败: {e}\n")));
                    fail += 1;
                    let _ = sender.send(TaskMessage::Progress(r as f32 / total as f32));
                    continue;
                }
            }
        }

        if !item.can_separate() {
            let _ = sender.send(TaskMessage::Log(format!(
                "\n=== {} ===\n无可处理音频, 跳过.\n",
                item.path.file_name().unwrap_or_default().to_string_lossy()
            )));
            let _ = sender.send(TaskMessage::Progress(r as f32 / total as f32));
            continue;
        }

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

        let cfg = find_separator(separators, separator_name);
        let cmd = if let Some(cfg) = cfg {
            build_command(cfg, demucs_base, output_dir, audio, model, mode, device)
        } else {
            let mut fc = demucs_base.to_vec();
            let m = if mode == "six_stems" && model != "htdemucs_6s" {
                "htdemucs_6s".to_string()
            } else {
                model.to_string()
            };
            fc.extend_from_slice(&[
                "-n".into(),
                m,
                "-o".into(),
                output_dir.to_string_lossy().to_string(),
            ]);
            if mode == "vocals" {
                fc.push("--two-stems=vocals".into());
            }
            if device != "auto" {
                fc.extend_from_slice(&["-d".into(), device.to_string()]);
            }
            fc.push(audio.to_string_lossy().to_string());
            fc
        };

        let cmd_str = cmd
            .iter()
            .map(|p| {
                if p.contains(' ') {
                    format!("\"{}\"", p)
                } else {
                    p.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let _ = sender.send(TaskMessage::Log(format!("{}\n", cmd_str)));

        let mut proc_env: std::collections::HashMap<String, String> = std::env::vars().collect();
        proc_env
            .entry("PYTHONIOENCODING".into())
            .or_insert_with(|| "utf-8".into());
        proc_env
            .entry("PYTHONUTF8".into())
            .or_insert_with(|| "1".into());

        match Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .envs(&proc_env)
            .spawn()
        {
            Ok(mut child) => {
                if let Some(out) = child.stdout.take() {
                    for line in BufReader::new(out).lines().flatten() {
                        let _ = sender.send(TaskMessage::Log(format!("{}\n", line)));
                        if !*running.lock().unwrap() {
                            let _ = child.kill();
                            break;
                        }
                    }
                }
                if let Some(err) = child.stderr.take() {
                    for line in BufReader::new(err).lines().flatten() {
                        let _ = sender.send(TaskMessage::Log(format!("{}\n", line)));
                    }
                }
                let exit = child.wait().unwrap_or_default();
                if exit.success() {
                    ok += 1;
                } else {
                    fail += 1;
                }
            }
            Err(e) => {
                fail += 1;
                let _ = sender.send(TaskMessage::Log(format!("启动失败: {e}\n")));
                break;
            }
        }
        let _ = sender.send(TaskMessage::Progress(r as f32 / total as f32));

        // stamp metadata after each track completes
        if let Some(ref src) = item.process_path {
            let n = stamp_metadata_for_source(src, output_dir);
            if n > 0 {
                let _ = sender.send(TaskMessage::Log(format!("Metadata stamped: {} stems\n", n)));
            }
        }
    }
    let _ = sender.send(TaskMessage::Status(format!("完成 {}, 失败 {}", ok, fail)));
    let _ = sender.send(TaskMessage::Done);
}

pub fn find_ffmpeg() -> Option<PathBuf> {
    find_tool("ffmpeg.exe")
}

pub fn transfer_metadata(source_audio: &Path, stem_wav: &Path) -> Option<PathBuf> {
    let ffmpeg = find_ffmpeg()?;
    let out = stem_wav.with_extension("flac");
    let mut cmd = Command::new(&ffmpeg);
    cmd.args([
        "-y",
        "-i",
        &stem_wav.to_string_lossy(),
        "-i",
        &source_audio.to_string_lossy(),
        "-map",
        "0:a",
        "-map_metadata",
        "1",
        "-map",
        "1:v?",
        "-c:a",
        "flac",
        "-disposition:v",
        "attached_pic",
        &out.to_string_lossy(),
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    #[cfg(windows)]
    {
        cmd.creation_flags(0x08000000);
    }
    let status = cmd.status().ok()?;
    if status.success() {
        Some(out)
    } else {
        None
    }
}

pub fn stamp_metadata_for_source(source_audio: &Path, output_dir: &Path) -> u32 {
    let mut count = 0u32;
    for entry in WalkDir::new(output_dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            if let Some(ext) = entry.path().extension() {
                if ext == "wav"
                    && entry
                        .path()
                        .parent()
                        .map(|p| p != output_dir)
                        .unwrap_or(false)
                {
                    if transfer_metadata(source_audio, entry.path()).is_some() {
                        count += 1;
                    }
                }
            }
        }
    }
    count
}
