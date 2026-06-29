use clap::Parser;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use yyw::{
    find_demucs, find_tool, load_separators, run_separation, scan_inputs, AudioItem, Settings,
    TaskMessage,
};

#[derive(Parser)]
#[command(name = "yyw-cli", about = "NCM conversion & stem separation CLI")]
struct Args {
    /// Input file or directory
    #[arg(short, long, default_value = r"G:\CloudMusic\VipSongsDownload")]
    input: String,

    /// Output directory for stems
    #[arg(short, long, default_value = "stems_output")]
    output: String,

    /// Separator tool (from separators.json)
    #[arg(long, default_value = "Demucs")]
    tool: String,

    /// Model name
    #[arg(short, long, default_value = "htdemucs")]
    model: String,

    /// Separation mode (vocals, four_stems, six_stems, etc.)
    #[arg(short = 'M', long, default_value = "vocals")]
    mode: String,

    /// Device (auto, cpu, cuda)
    #[arg(short, long, default_value = "auto")]
    device: String,

    /// Only convert NCM files, don't separate
    #[arg(long)]
    convert_only: bool,
}

fn main() {
    let args = Args::parse();
    let output_dir = PathBuf::from(&args.output);
    let _ = std::fs::create_dir_all(&output_dir);

    let ncmdump_path = find_tool("ncmdump.exe");
    let ncmdump_avail = ncmdump_path.is_some();
    let demucs_cmd = find_demucs();
    let separators = load_separators();

    eprintln!("yyw-cli - NCM conversion & stem separation");
    eprintln!(
        "  Tool: {}  Model: {}  Mode: {}  Device: {}",
        args.tool, args.model, args.mode, args.device
    );

    // scan input
    let input_path = PathBuf::from(&args.input);
    let mut items: Vec<AudioItem> = Vec::new();
    if input_path.is_dir() {
        // treat as source directory
        let src = input_path.to_string_lossy().to_string();
        scan_inputs(&src, &mut items);
        eprintln!("  Scanned {} files from {}", items.len(), src);

        // Save scanned settings
        let s = Settings {
            source: src,
            output: args.output.clone(),
            model: args.model.clone(),
            mode: args.mode.clone(),
            device: args.device.clone(),
            separator: args.tool.clone(),
        };
        s.save();
    } else if input_path.is_file() {
        let is_ncm = input_path.extension().map(|e| e == "ncm").unwrap_or(false);
        items.push(AudioItem {
            path: input_path.clone(),
            status: "待处理".into(),
            process_path: if is_ncm {
                None
            } else {
                Some(input_path.clone())
            },
            kind: if is_ncm {
                yyw::AudioKind::Ncm
            } else {
                yyw::AudioKind::Normal
            },
        });
    } else {
        eprintln!("Error: input path does not exist: {}", args.input);
        std::process::exit(1);
    }

    if items.is_empty() {
        eprintln!("No audio files found.");
        std::process::exit(0);
    }

    // Convert-only mode
    if args.convert_only {
        for item in &items {
            if matches!(item.kind, yyw::AudioKind::Ncm) {
                if let Some(ref np) = ncmdump_path {
                    eprintln!("Converting: {}", item.path.display());
                    match yyw::convert_ncm_sync(
                        np,
                        &item.path,
                        item.path.parent().unwrap_or(std::path::Path::new(".")),
                    ) {
                        Ok(c) => eprintln!("  -> {}", c.display()),
                        Err(e) => eprintln!("  FAIL: {e}"),
                    }
                } else {
                    eprintln!("ncmdump not found, cannot convert: {}", item.path.display());
                }
            } else {
                eprintln!("Not NCM, skip: {}", item.path.display());
            }
        }
        return;
    }

    // Run separation
    let total = items.len();
    eprintln!("Processing {} files...", total);

    let running = Arc::new(Mutex::new(true));
    let (tx, rx) = mpsc::channel();

    let items_clone = items.clone();
    let output_dir_clone = output_dir.clone();
    let separators_clone = separators.clone();
    let tool = args.tool.clone();
    let model = args.model.clone();
    let mode = args.mode.clone();
    let device = args.device.clone();
    let demucs_cmd_clone = demucs_cmd.clone();
    let np_clone = ncmdump_path.clone();
    let running_clone = running.clone();

    thread::spawn(move || {
        run_separation(
            ncmdump_avail,
            &np_clone,
            &items_clone,
            &output_dir_clone,
            &separators_clone,
            &tool,
            &model,
            &mode,
            &device,
            &demucs_cmd_clone,
            tx,
            running_clone,
        );
    });

    // Print progress to stderr, logs to stdout
    loop {
        match rx.recv() {
            Ok(msg) => match msg {
                TaskMessage::Log(text) => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                }
                TaskMessage::Progress(p) => {
                    eprint!("\rProgress: {:.0}%", p * 100.0);
                    let _ = std::io::stderr().flush();
                }
                TaskMessage::Status(s) => {
                    eprintln!("\r{}", s);
                }
                TaskMessage::InputStatus(_idx, _status, _pp) => {}
                TaskMessage::Done => break,
            },
            Err(_) => break,
        }
    }

    *running.lock().unwrap() = false;
    eprintln!("\nDone. Output: {}", output_dir.display());
}
