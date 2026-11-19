//! Local SenseVoice transcription engine: model download/verify, resident recognition thread, 16 kHz PCM transcription.
//! Shared with Codex++'s local dictation (model dir also shared: under `$CODEX_HOME` or `~/.codex`
//! `speech-to-text/sensevoice-onnx`, both programs download once).

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Max audio bytes per transcription (matches Codex++'s dictation API).
pub const MAX_AUDIO_BODY_BYTES: usize = 25 * 1024 * 1024;
const PCM_WAV_HEADER_BYTES: usize = 44;
static LOCAL_RECOGNIZER_SLOTS: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(1)));
static LOCAL_DOWNLOAD_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static LOCAL_DOWNLOAD_STATE: LazyLock<Mutex<Value>> =
    LazyLock::new(|| Mutex::new(json!({ "phase": "idle" })));


const SENSEVOICE_MODEL_URL: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/model.int8.onnx";
const SENSEVOICE_MODEL_BYTES: u64 = 239_233_841;
const SENSEVOICE_MODEL_SHA256: &str =
    "c71f0ce00bec95b07744e116345e33d8cbbe08cef896382cf907bf4b51a2cd51";
const SENSEVOICE_TOKENS_URL: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17/resolve/2365baeacb507f821a0c8120fcee3d484dba7a07/tokens.txt";
const SENSEVOICE_TOKENS_BYTES: u64 = 315_894;
const SENSEVOICE_TOKENS_SHA256: &str =
    "f449eb28dc567533d7fa59be34e2abca8784f771850c78a47fb731a31429a1dc";

/// Transcription failure: `status` expresses category via HTTP status code (400 input, 403 disabled, 429 busy, 503 model not ready, etc.).
#[derive(Debug, Clone)]
pub struct EngineError {
    pub status: u16,
    pub message: String,
}

impl EngineError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self { status, message: message.into() }
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

/// Model directory: `$CODEX_HOME` (if absolute path) or `~/.codex` under `speech-to-text/sensevoice-onnx`.
pub fn model_dir() -> PathBuf {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute() && path.is_dir())
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.home_dir().join(".codex")))
        .unwrap_or_else(|| PathBuf::from(".codex"));
    home.join("speech-to-text").join("sensevoice-onnx")
}

/// Only checks file size (not SHA-256): for status display; full verification happens before actual load.
pub fn model_ready() -> bool {
    let root = model_dir();
    SENSEVOICE_ASSETS.iter().all(|asset| {
        std::fs::metadata(root.join(asset.name))
            .is_ok_and(|meta| meta.is_file() && meta.len() == asset.bytes)
    })
}
#[derive(Clone, Copy)]
struct LocalAsset {
    name: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
}

const SENSEVOICE_ASSETS: [LocalAsset; 2] = [
    LocalAsset {
        name: "model.int8.onnx",
        url: SENSEVOICE_MODEL_URL,
        bytes: SENSEVOICE_MODEL_BYTES,
        sha256: SENSEVOICE_MODEL_SHA256,
    },
    LocalAsset {
        name: "tokens.txt",
        url: SENSEVOICE_TOKENS_URL,
        bytes: SENSEVOICE_TOKENS_BYTES,
        sha256: SENSEVOICE_TOKENS_SHA256,
    },
];

fn local_asset_verified(path: &std::path::Path, asset: &LocalAsset) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() != asset.bytes {
        return false;
    }
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        match std::io::Read::read(&mut file, &mut buffer) {
            Ok(0) => break,
            Ok(length) => digest.update(&buffer[..length]),
            Err(_) => return false,
        }
    }
    format!("{:x}", digest.finalize()) == asset.sha256
}

/// Config page only reads cached state, doesn't trigger download.
pub fn model_status() -> Value {
    let mut state = LOCAL_DOWNLOAD_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    state["modelReady"] = json!(model_ready());
    state["totalBytes"] = json!(SENSEVOICE_MODEL_BYTES + SENSEVOICE_TOKENS_BYTES);
    state
}

fn publish_local_download(state: Value) {
    *LOCAL_DOWNLOAD_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = state;
}

/// Only manager's manual download button calls this; transcription entrypoint never calls this.
pub async fn prepare_model() -> Result<Value, EngineError> {
    let _guard = LOCAL_DOWNLOAD_LOCK
        .try_lock()
        .map_err(|_| EngineError::new(409, "Local speech model downloading, please wait"))?;
    publish_local_download(json!({ "phase": "downloading", "completedBytes": 0 }));
    let result = async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3_600))
            .user_agent("ClaudeLocal SenseVoice/1.0")
            .build()
            .map_err(|_| EngineError::new(502, "Failed to create local speech model connection"))?;
        let mut completed = 0;
        for asset in &SENSEVOICE_ASSETS {
            download_local_asset(&client, asset, completed, &model_dir()).await?;
            completed += asset.bytes;
        }
        Ok::<(), EngineError>(())
    }
    .await;
    match result {
        Ok(()) => {
            publish_local_download(
                json!({ "phase": "ready", "completedBytes": SENSEVOICE_MODEL_BYTES + SENSEVOICE_TOKENS_BYTES }),
            );
            Ok(model_status())
        }
        Err(error) => {
            publish_local_download(json!({ "phase": "failed", "message": error.message }));
            Err(error)
        }
    }
}

/// Check local cache for fixed size and SHA-256; only errors on missing/corrupt, doesn't create dir or go online.
fn verified_local_assets(root: &std::path::Path) -> Result<Vec<PathBuf>, EngineError> {
    SENSEVOICE_ASSETS
        .iter()
        .map(|asset| {
            let path = root.join(asset.name);
            if local_asset_verified(&path, asset) {
                Ok(path)
            } else {
                Err(EngineError::new(
                    400,
                    "SenseVoice model missing or verification failed, please download manually in voice input config page",
                ))
            }
        })
        .collect()
}

async fn download_local_asset(
    client: &reqwest::Client,
    asset: &LocalAsset,
    completed: u64,
    root: &std::path::Path,
) -> Result<PathBuf, EngineError> {
    publish_local_download(
        json!({ "phase": "downloading", "resource": asset.name, "completedBytes": completed }),
    );
    tokio::fs::create_dir_all(root)
        .await
        .map_err(|_| EngineError::new(500, "Failed to create local speech model directory"))?;
    let destination = root.join(asset.name);
    let verified = tokio::task::spawn_blocking({
        let destination = destination.clone();
        let asset = *asset;
        move || local_asset_verified(&destination, &asset)
    })
    .await
    .unwrap_or(false);
    if verified {
        return Ok(destination);
    }
    let partial = root.join(format!("{}.{}.part", asset.name, uuid::Uuid::new_v4()));
    let mirror_url = asset
        .url
        .replacen("https://huggingface.co", "https://hf-mirror.com", 1);
    let mut last_error = None;
    for url in [asset.url.to_string(), mirror_url] {
        let response = match client.get(&url).send().await {
            Ok(response)
                if response.status().is_success()
                    && response
                        .content_length()
                        .is_none_or(|length| length == asset.bytes) =>
            {
                response
            }
            Ok(response) => {
                last_error = Some(format!("HTTP {}", response.status().as_u16()));
                continue;
            }
            Err(_) => {
                last_error = Some("Cannot connect to model download source".to_string());
                continue;
            }
        };
        let mut file = match tokio::fs::File::create(&partial).await {
            Ok(file) => file,
            Err(_) => return Err(EngineError::new(500, "Failed to write local speech model cache")),
        };
        let mut digest = Sha256::new();
        let mut bytes = 0u64;
        let mut stream = response.bytes_stream();
        let mut failed = None;
        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(_) => {
                    failed = Some("Model download interrupted, please retry".to_string());
                    break;
                }
            };
            bytes = bytes.saturating_add(chunk.len() as u64);
            if bytes > asset.bytes {
                failed = Some("Model file size exceeds expected".to_string());
                break;
            }
            if tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                .await
                .is_err()
            {
                failed = Some("Failed to write model file".to_string());
                break;
            }
            digest.update(&chunk);
            publish_local_download(
                json!({ "phase": "downloading", "resource": asset.name, "completedBytes": completed + bytes }),
            );
        }
        if failed.is_none()
            && (bytes != asset.bytes || format!("{:x}", digest.finalize()) != asset.sha256)
        {
            failed = Some("Model file integrity check failed".to_string());
        }
        // tokio's write_all only hands data to background thread; drop/rename without flush may leave file half-written:
        // published model would be partial, subsequent verification fails on Windows (CI flaky).
        if failed.is_none() && tokio::io::AsyncWriteExt::flush(&mut file).await.is_err() {
            failed = Some("Failed to write model file".to_string());
        }
        drop(file);
        if failed.is_none() {
            if tokio::fs::rename(&partial, &destination).await.is_ok() {
                return Ok(destination);
            }
            failed = Some("Failed to publish model cache file".to_string());
        }
        let _ = tokio::fs::remove_file(&partial).await;
        last_error = failed;
    }
    Err(EngineError::new(
        502,
        format!(
            "Local SenseVoice model download failed{}",
            last_error.map(|e| format!(": {e}")).unwrap_or_default()
        ),
    ))
}

fn validate_local_wav(audio: &[u8]) -> Result<(), EngineError> {
    if audio.len() < 44 || &audio[..4] != b"RIFF" || &audio[8..12] != b"WAVE" {
        return Err(EngineError::new(400, "Local SenseVoice requires WAV audio"));
    }
    if u16::from_le_bytes([audio[20], audio[21]]) != 1
        || u16::from_le_bytes([audio[22], audio[23]]) != 1
        || u32::from_le_bytes([audio[24], audio[25], audio[26], audio[27]]) != 16_000
        || u16::from_le_bytes([audio[34], audio[35]]) != 16
    {
        return Err(EngineError::new(
            400,
            "Local SenseVoice requires 16 kHz mono PCM WAV",
        ));
    }
    Ok(())
}

/// Process-cached local SenseVoice recognizer.
///
/// Previously each transcription recreated `OfflineRecognizer::create`, measured 9-18s even for 1s audio, and latency
/// almost independent of audio length — bottleneck is 239 MB model loading and feature extractor init, not decoding.
/// Reuse same recognizer when model and language unchanged; single transcription left with just decode+transfer; cost is resident model memory.
struct CachedLocalRecognizer {
    model: PathBuf,
    tokens: PathBuf,
    language: String,
    /// Each asset's (size, mtime); any change triggers recognizer rebuild.
    signature: ((u64, u64), (u64, u64)),
    recognizer: sherpa_onnx::OfflineRecognizer,
}

/// Local recognition dedicated thread.
///
/// sherpa's `OfflineRecognizer` manually marked `Send`/`Sync` by crate, but internally C++ handle + custom thread pool.
/// Observed helper process silent exit under high-frequency reuse (no crash report), while single-threaded reuse stable.
/// So put both loading and decoding on same thread: avoids cross-thread FFI handle access, naturally queues concurrent requests serial, no longer depends on tokio blocking pool scheduling.
enum LocalRecognizerRequest {
    Transcribe {
        samples: Vec<f32>,
        language: String,
        reply: std::sync::mpsc::Sender<Result<String, EngineError>>,
    },
}

static LOCAL_RECOGNIZER_TX: LazyLock<Option<std::sync::mpsc::Sender<LocalRecognizerRequest>>> =
    LazyLock::new(|| {
        let (sender, receiver) = std::sync::mpsc::channel::<LocalRecognizerRequest>();
        std::thread::Builder::new()
            .name("claude-local-dictation".to_string())
            .spawn(move || local_recognizer_loop(receiver))
            .map(|_| sender)
            .ok()
    });

fn local_recognizer_loop(receiver: std::sync::mpsc::Receiver<LocalRecognizerRequest>) {
    let mut cached: Option<CachedLocalRecognizer> = None;
    while let Ok(LocalRecognizerRequest::Transcribe {
        samples,
        language,
        reply,
    }) = receiver.recv()
    {
        let result = recognize_local(&mut cached, &samples, &language);
        // Ignore send failure when caller gone: this thread only responsible for returning result.
        let _ = reply.send(result);
    }
}

/// SenseVoice only consumes 16 kHz mono, this value also validated by `validate_local_wav` sample rate.
const LOCAL_SAMPLE_RATE: i32 = 16_000;

/// Lightweight file fingerprint: loaded model reused as long as size and mtime unchanged.
fn local_asset_signature(path: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some((meta.len(), modified))
}

/// 16 kHz mono PCM to f32 samples; `validate_local_wav` already confirmed header format.
fn local_wav_samples(audio: &[u8]) -> Vec<f32> {
    audio[44..]
        .chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32_768.0)
        .collect()
}

fn spawn_local_recognizer_waiter<T: Send + 'static>(
    answer: std::sync::mpsc::Receiver<T>,
    guards: Vec<Arc<OwnedSemaphorePermit>>,
) -> tokio::task::JoinHandle<Result<T, std::sync::mpsc::RecvError>> {
    tokio::task::spawn_blocking(move || {
        let result = answer.recv();
        drop(guards);
        result
    })
}

/// Run local recognition once in recognition thread; reuse cache when model unchanged, no reload, no SHA256 recompute.
fn recognize_local(
    cached: &mut Option<CachedLocalRecognizer>,
    samples: &[f32],
    language: &str,
) -> Result<String, EngineError> {
    let root = model_dir();
    let model = root.join(SENSEVOICE_ASSETS[0].name);
    let tokens = root.join(SENSEVOICE_ASSETS[1].name);
    let signature = local_asset_signature(&model).zip(local_asset_signature(&tokens));
    let reusable = match (&*cached, signature) {
        (Some(entry), Some(signature)) => {
            entry.model == model
                && entry.tokens == tokens
                && entry.language == language
                && entry.signature == signature
        }
        _ => false,
    };
    if !reusable {
        // Only full verification when actually loading: 239 MB SHA256 shouldn't be on every transcription path.
        verified_local_assets(&root)?;
        let mut config = sherpa_onnx::OfflineRecognizerConfig::default();
        config.model_config.sense_voice = sherpa_onnx::OfflineSenseVoiceModelConfig {
            model: Some(model.to_string_lossy().into_owned()),
            language: Some(language.to_string()),
            use_itn: true,
        };
        config.model_config.tokens = Some(tokens.to_string_lossy().into_owned());
        config.model_config.provider = Some("cpu".to_string());
        config.model_config.num_threads = 2;
        let recognizer = sherpa_onnx::OfflineRecognizer::create(&config)
            .ok_or_else(|| EngineError::new(502, "SenseVoice model load failed"))?;
        *cached = Some(CachedLocalRecognizer {
            model,
            tokens,
            language: language.to_string(),
            signature: signature.unwrap_or_default(),
            recognizer,
        });
    }
    let entry = cached
        .as_ref()
        .ok_or_else(|| EngineError::new(500, "Local speech model not ready"))?;
    let stream = entry.recognizer.create_stream();
    // Feed samples directly, no more temp WAV per transcription.
    stream.accept_waveform(LOCAL_SAMPLE_RATE, samples);
    entry.recognizer.decode(&stream);
    Ok(stream
        .get_result()
        .map(|result| result.text)
        .unwrap_or_default()
        .trim()
        .to_string())
}

/// Transcribe a 16 kHz mono WAV in recognition thread. `language` already a SenseVoice-supported value.
async fn transcribe_wav(
    audio: &[u8],
    language: &str,
    stream_guard: Option<Arc<OwnedSemaphorePermit>>,
) -> Result<String, EngineError> {
    validate_local_wav(audio)?;
    let recognizer_guard = LOCAL_RECOGNIZER_SLOTS
        .clone()
        .try_acquire_owned()
        .map_err(|_| EngineError::new(429, "Local speech recognition busy, please retry"))?;
    let sender = LOCAL_RECOGNIZER_TX
        .as_ref()
        .ok_or_else(|| EngineError::new(500, "Local speech inference thread unavailable"))?;
    let (reply, answer) = std::sync::mpsc::channel();
    sender
        .send(LocalRecognizerRequest::Transcribe {
            samples: local_wav_samples(audio),
            language: language.to_string(),
            reply,
        })
        .map_err(|_| EngineError::new(500, "Local speech inference thread stopped"))?;
    // Segmented decode shares session permit; socket disconnect must wait for non-cancellable inference to complete before releasing capacity.
    let mut guards = vec![Arc::new(recognizer_guard)];
    if let Some(stream_guard) = stream_guard {
        guards.push(stream_guard);
    }
    spawn_local_recognizer_waiter(answer, guards)
        .await
        .map_err(|_| EngineError::new(500, "Local SenseVoice inference failed"))?
        .map_err(|_| EngineError::new(500, "Local speech inference thread stopped"))?
}

/// Validate and transcribe WAV (`language` empty = auto).
pub async fn transcribe_wav_audio(audio: &[u8], language: &str) -> Result<String, EngineError> {
    let language = normalize_language(language)?;
    transcribe_wav(audio, &language, None).await
}

fn normalize_language(language: &str) -> Result<String, EngineError> {
    match language.trim() {
        "" => Ok("auto".to_string()),
        value @ ("auto" | "zh" | "en" | "yue" | "ja" | "ko") => Ok(value.to_string()),
        _ => Err(EngineError::new(400, "SenseVoice doesn't support that speech language")),
    }
}

fn pcm16_mono_16khz_wav(pcm: &[u8]) -> Vec<u8> {
    let mut wav = Vec::with_capacity(PCM_WAV_HEADER_BYTES + pcm.len());
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

fn validate_local_pcm16(pcm: &[u8]) -> Result<(), EngineError> {
    if pcm.is_empty() || pcm.len() % 2 != 0 {
        return Err(EngineError::new(400, "PCM audio empty or invalid length"));
    }
    if pcm.len() > MAX_AUDIO_BODY_BYTES - PCM_WAV_HEADER_BYTES {
        return Err(EngineError::new(413, "PCM audio exceeds 25 MiB limit"));
    }
    Ok(())
}

/// Transcribe 16 kHz mono 16-bit PCM; `stream_guard` is streaming session permit, released after inference completes.
pub async fn transcribe_pcm16(
    pcm: &[u8],
    language: Option<&str>,
    stream_guard: Option<Arc<OwnedSemaphorePermit>>,
) -> Result<String, EngineError> {
    if !model_ready() {
        return Err(EngineError::new(503, "Local SenseVoice model not ready"));
    }
    validate_local_pcm16(pcm)?;
    let language = normalize_language(language.unwrap_or_default())?;
    let wav = pcm16_mono_16khz_wav(pcm);
    transcribe_wav(&wav, &language, stream_guard).await
}