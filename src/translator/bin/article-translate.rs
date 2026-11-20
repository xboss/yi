#[cfg(any(feature = "native-ct2", feature = "local-chat"))]
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
};
#[cfg(any(feature = "native-ct2", feature = "local-chat"))]
use translator::{Backend, Document, GlossaryEntry, GlossaryMode, Language, Progress, Translator};

const HELP: &str = r#"Translate a local plain-text or Markdown article.

article-translate --backend ct2 --family lmt --model-dir DIR --input article.md --output article.zh.md
article-translate --backend chat --model MODEL --input article.md --output article.zh.md

Options:
  --backend ct2|chat         Required; ct2 needs the native-ct2 Cargo feature
  --family lmt|nllb|opus-en-zh|opus-zh-en  Native model format (default lmt)
  --model-dir DIR            Directory containing model.bin
  --model NAME               Already-loaded local chat model
  --base-url URL             Chat API base (default http://127.0.0.1:11434/v1)
  --from LANG --to LANG      Language tags (default en / zh-Hans)
  --format markdown|text    Default inferred from .md/.markdown extension
  --input PATH --output PATH Required; output must not already exist
  --report PATH              Optional complete JSON report; must not exist
  --progress-file PATH       Optional JSONL of completed blocks; must not exist
  --glossary PATH            JSON array of {"source":"...","target":"..."}
  --glossary-mode audit|prompt  Default audit; prompt needs chat support
  --context N               Reference blocks; default 2 for chat, 0 for ct2
  --max-input N             Max tokens per model call (including prompt/context)
  --max-output N            Generated token budget (default 1024; seq2seq 512)
  --window N                Chat server's actual context, or LMT window override
  --beam N                  Native beam search size (default 5); chat accepts 1
  --threads N               Native CPU threads (default at most 8)
  --compute default|float32|int8  Native compute type; default preserves stored precision
  --retry-splits N          Splits after truncated/malformed output (default 2)
  --help                    Show this help

The program loads local files/models and writes new files only. It does not
download models or start a server. Cancellation is available through the Rust
API. API key for a local server, if needed: TRANSLATOR_API_KEY.
"#;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

#[cfg(not(any(feature = "native-ct2", feature = "local-chat")))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        print!("{HELP}");
        Ok(())
    } else {
        Err("enable the local-chat or native-ct2 feature for the CLI".into())
    }
}

#[cfg(any(feature = "native-ct2", feature = "local-chat"))]
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = BTreeMap::new();
    let mut iter = std::env::args().skip(1);
    while let Some(key) = iter.next() {
        if key == "--help" || key == "-h" {
            print!("{HELP}");
            return Ok(());
        }
        let allowed = [
            "--backend",
            "--family",
            "--model-dir",
            "--model",
            "--base-url",
            "--from",
            "--to",
            "--format",
            "--input",
            "--output",
            "--report",
            "--progress-file",
            "--glossary",
            "--glossary-mode",
            "--context",
            "--max-input",
            "--max-output",
            "--window",
            "--beam",
            "--threads",
            "--compute",
            "--retry-splits",
        ];
        if !allowed.contains(&key.as_str()) {
            return Err(format!("unknown option {key}; use --help").into());
        }
        let value = iter
            .next()
            .filter(|v| !v.starts_with("--"))
            .ok_or_else(|| format!("missing value for {key}"))?;
        if args.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate option {key}").into());
        }
    }
    let required = |key: &str| {
        args.get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("missing {key}; use --help"))
    };
    let number = |key: &str| -> Result<Option<usize>, Box<dyn std::error::Error>> {
        args.get(key)
            .map(|v| {
                v.parse()
                    .map_err(|_| format!("{key} must be a nonnegative integer").into())
            })
            .transpose()
    };
    let input_path = required("--input")?;
    let output_path = required("--output")?;
    let input = fs::read_to_string(input_path)?;
    let mut destinations = Vec::new();
    for key in ["--output", "--report", "--progress-file"] {
        if let Some(path) = args.get(key) {
            if Path::new(path).exists() {
                return Err(format!("{key} already exists: {path}").into());
            }
            let parent = Path::new(path)
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let name = Path::new(path)
                .file_name()
                .ok_or("output filename is missing")?;
            let absolute = parent.canonicalize()?.join(name);
            if destinations.contains(&absolute) {
                return Err("output, report and progress paths must differ".into());
            }
            destinations.push(absolute);
        }
    }
    let from = args
        .get("--from")
        .map_or("en", String::as_str)
        .parse::<Language>()?;
    let to = args
        .get("--to")
        .map_or("zh-Hans", String::as_str)
        .parse::<Language>()?;
    let backend_name = required("--backend")?;
    let backend: Box<dyn Backend> = match backend_name {
        "chat" => {
            #[cfg(feature = "local-chat")]
            {
                use translator::backends::local_chat::{LocalChatBackend, LocalChatConfig};
                for key in ["--family", "--model-dir", "--threads", "--compute"] {
                    if args.contains_key(key) {
                        return Err(format!("{key} is a native-only option").into());
                    }
                }
                let mut config = LocalChatConfig::new(required("--model")?);
                if let Some(url) = args.get("--base-url") {
                    config = config.base_url(url);
                }
                if let Ok(key) = std::env::var("TRANSLATOR_API_KEY") {
                    config = config.api_key(key);
                }
                if let Some(n) = number("--window")? {
                    config = config.context_window_tokens(n);
                }
                if let Some(n) = number("--max-input")? {
                    config = config.max_input_tokens(n);
                }
                if let Some(n) = number("--max-output")? {
                    config = config.max_output_tokens(n);
                }
                Box::new(LocalChatBackend::new(config)?)
            }
            #[cfg(not(feature = "local-chat"))]
            {
                return Err("build with the local-chat feature".into());
            }
        }
        "ct2" => {
            #[cfg(feature = "native-ct2")]
            {
                use translator::backends::ct2::{ComputeType, Ct2Backend, Ct2Config, ModelFamily};
                for key in ["--model", "--base-url"] {
                    if args.contains_key(key) {
                        return Err(format!("{key} is a chat-only option").into());
                    }
                }
                let family = match args.get("--family").map_or("lmt", String::as_str) {
                    "lmt" => ModelFamily::Lmt,
                    "nllb" => ModelFamily::Nllb,
                    "opus-en-zh" => ModelFamily::OpusEnZh,
                    "opus-zh-en" => ModelFamily::OpusZhEn,
                    _ => return Err("unknown --family".into()),
                };
                let mut config = Ct2Config::new(required("--model-dir")?, family);
                if let Some(n) = number("--threads")? {
                    config = config.threads(n);
                }
                if let Some(n) = number("--max-input")? {
                    config = config.max_input_tokens(n);
                }
                if let Some(n) = number("--max-output")? {
                    config = config.max_output_tokens(n);
                }
                if let Some(n) = number("--window")? {
                    config = config.context_window_tokens(n);
                }
                config = config.compute_type(
                    match args.get("--compute").map_or("default", String::as_str) {
                        "default" => ComputeType::Default,
                        "float32" => ComputeType::Float32,
                        "int8" => ComputeType::Int8Float32,
                        _ => return Err("unknown --compute".into()),
                    },
                );
                Box::new(Ct2Backend::load(config)?)
            }
            #[cfg(not(feature = "native-ct2"))]
            {
                return Err("build with --features native-ct2 to use local model files".into());
            }
        }
        _ => return Err("--backend must be ct2 or chat".into()),
    };
    let caps = backend.capabilities();
    let mut decoding = translator::DecodingOptions {
        beam_size: if caps.supports_beam_search { 5 } else { 1 },
        max_output_tokens: 1024.min(caps.max_output_tokens),
        ..Default::default()
    };
    if let Some(n) = number("--beam")? {
        decoding.beam_size = n;
    }
    if let Some(n) = number("--max-output")? {
        decoding.max_output_tokens = n;
    }
    let mut builder = Translator::builder(backend, from, to).decoding(decoding);
    if let Some(n) = number("--max-input")? {
        builder = builder.max_chunk_tokens(n);
    }
    if let Some(n) = number("--context")? {
        builder = builder.context_paragraphs(n);
    }
    if let Some(n) = number("--retry-splits")? {
        builder = builder.retry_splits(n);
    }
    if let Some(path) = args.get("--glossary") {
        let glossary: Vec<GlossaryEntry> = serde_json::from_str(&fs::read_to_string(path)?)?;
        builder = builder.glossary(glossary);
    }
    builder = builder.glossary_mode(
        match args.get("--glossary-mode").map_or("audit", String::as_str) {
            "audit" => GlossaryMode::AuditOnly,
            "prompt" => GlossaryMode::PromptAndAudit,
            _ => return Err("--glossary-mode must be audit or prompt".into()),
        },
    );
    let mut translator = builder.build()?;
    let format = args.get("--format").map(String::as_str).unwrap_or_else(|| {
        if matches!(
            Path::new(input_path).extension().and_then(|v| v.to_str()),
            Some("md" | "markdown")
        ) {
            "markdown"
        } else {
            "text"
        }
    });
    let document = match format {
        "markdown" => Document::markdown(input),
        "text" => Document::plain(input),
        _ => return Err("unknown --format".into()),
    };
    let mut progress_file = args
        .get("--progress-file")
        .map(|path| OpenOptions::new().write(true).create_new(true).open(path))
        .transpose()?;
    let mut progress_error = None;
    let report = translator.translate_with_progress(&document, |event| match event {
        Progress::Started { total_blocks } => {
            eprintln!("Translating {total_blocks} blocks with {}", caps.name)
        }
        Progress::BlockFinished(block) => {
            eprintln!("Completed block {}", block.index + 1);
            if let Some(file) = &mut progress_file {
                let result = (|| -> io::Result<()> {
                    serde_json::to_writer(&mut *file, block)?;
                    file.write_all(b"\n")?;
                    file.flush()
                })();
                if let Err(error) = result {
                    progress_error = Some(error);
                }
            }
        }
        Progress::Retrying { index, reason } => eprintln!("Retrying block {}: {reason}", index + 1),
        _ => {}
    })?;
    if let Some(error) = progress_error {
        return Err(error.into());
    }
    // Serialize before opening output files; an encoding error cannot leave an
    // apparently valid empty output. create_new prevents accidental overwrite.
    let report_json = serde_json::to_vec_pretty(&report)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)?
        .write_all(report.text.as_bytes())?;
    if let Some(path) = args.get("--report") {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(&report_json)?;
    }
    for diagnostic in &report.diagnostics {
        eprintln!("{:?}: {}", diagnostic.kind, diagnostic.message);
    }
    eprintln!(
        "Saved {} blocks, {} model calls to {output_path}",
        report.blocks.len(),
        report.model_calls
    );
    Ok(())
}
