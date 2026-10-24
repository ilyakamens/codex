//! OpenAI dictation supplies user input; it never creates an assistant response.

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::legacy_core::config::Config;
use codex_login::AuthManager;
use codex_protocol::ThreadId;
use codex_websocket_client::WebSocketConnector;
use color_eyre::eyre::Context;
use color_eyre::eyre::Result;
use color_eyre::eyre::bail;
use futures::SinkExt;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;
use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncBufReadExt;
use tokio::io::AsyncWriteExt;
use tokio::io::BufReader;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;

#[derive(Debug)]
pub(crate) enum DictationEvent {
    Ready,
    Transcript { utterance_id: String, text: String },
    Error(String),
}

pub(crate) struct DictationHandle {
    task: JoinHandle<()>,
    controls: mpsc::UnboundedSender<bool>,
    peak: Arc<AtomicU16>,
}

impl Drop for DictationHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl DictationHandle {
    pub(crate) fn start(
        config: Config,
        microphone: Option<String>,
        thread_id: ThreadId,
        attempt_id: u64,
        tx: AppEventSender,
    ) -> Self {
        let (controls, receiver) = mpsc::unbounded_channel();
        let peak = Arc::new(AtomicU16::new(0));
        let task_peak = Arc::clone(&peak);
        let task = tokio::spawn(async move {
            let emit = |event| {
                tx.send(AppEvent::Dictation {
                    thread_id,
                    attempt_id,
                    event,
                });
            };
            if let Err(error) = run(config, microphone, receiver, task_peak, &emit).await {
                emit(DictationEvent::Error(format!(
                    "Dictation failed: {error:#}"
                )));
            }
        });
        Self {
            task,
            controls,
            peak,
        }
    }

    pub(crate) fn set_muted(&self, muted: bool) {
        let _ = self.controls.send(muted);
    }

    pub(crate) fn take_peak(&self) -> u16 {
        self.peak.swap(0, Ordering::Relaxed)
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum CaptureEvent {
    Ready,
    Audio {
        audio: String,
        peak: u16,
        rms: u16,
        samples: usize,
    },
    Error {
        message: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ServerEvent {
    #[serde(rename = "session.started")]
    Started,
    #[serde(rename = "session.updated")]
    Updated { session: SessionState },
    #[serde(rename = "transcript.final")]
    Transcript { utterance_id: String, text: String },
    #[serde(rename = "session.error", alias = "transcript.failed")]
    Error { error: DictationError },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct SessionState {
    status: String,
}

#[derive(Deserialize)]
struct DictationError {
    code: String,
    message: String,
}

// Keep a short prefix so quiet initial consonants survive the energy gate.
const PREFIX_SAMPLES: usize = 16000 * 3 / 10;
const PAUSE_SAMPLES: usize = 16000 * 12 / 10;
const SPEECH_RMS: u16 = 100;

#[derive(Default)]
struct UtteranceBuffer {
    frames: VecDeque<(String, usize)>,
    samples: usize,
    silence: usize,
    speaking: bool,
}

impl UtteranceBuffer {
    fn append(&mut self, audio: String, rms: u16, samples: usize) -> Option<Vec<String>> {
        self.frames.push_back((audio, samples));
        self.samples += samples;
        if rms >= SPEECH_RMS {
            self.speaking = true;
            self.silence = 0;
        } else {
            self.silence += samples;
        }
        if self.speaking {
            if self.silence >= PAUSE_SAMPLES {
                self.samples = 0;
                self.silence = 0;
                self.speaking = false;
                return Some(self.frames.drain(..).map(|(audio, _)| audio).collect());
            }
        } else {
            while self.samples > PREFIX_SAMPLES
                && let Some((_, samples)) = self.frames.pop_front()
            {
                self.samples -= samples;
            }
        }
        None
    }
}

async fn run(
    config: Config,
    microphone: Option<String>,
    mut controls: mpsc::UnboundedReceiver<bool>,
    peak: Arc<AtomicU16>,
    emit: &(impl Fn(DictationEvent) + Sync),
) -> Result<()> {
    let manager = AuthManager::shared_from_auth_config(config.auth_config(), false).await?;
    let (auth, factory) = manager
        .auth_with_http_client_factory()
        .await
        .ok_or_else(|| color_eyre::eyre::eyre!("Sign in to ChatGPT to use dictation"))?;
    if !auth.is_chatgpt_auth() {
        bail!("Sign in to ChatGPT to use dictation");
    }
    let base = config.chatgpt_base_url.trim_end_matches('/');
    let url = format!("{base}/dictation/stream?dictation_surface=composer")
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let mut request = url.into_client_request()?;
    let mut token = HeaderValue::from_str(&format!("Bearer {}", auth.get_token()?))?;
    token.set_sensitive(true);
    request.headers_mut().insert("Authorization", token);
    if let Some(account) = auth.get_account_id() {
        request
            .headers_mut()
            .insert("ChatGPT-Account-Id", HeaderValue::from_str(&account)?);
    }
    // This endpoint uses the desktop dictation protocol and its surface headers.
    request
        .headers_mut()
        .insert("originator", HeaderValue::from_static("Codex Desktop"));
    request.headers_mut().insert(
        "User-Agent",
        HeaderValue::from_static(concat!("codex_cli_rs/", env!("CARGO_PKG_VERSION"))),
    );
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_static("chatgpt-dictation, codex-desktop"),
    );
    let factory = config
        .http_client_factory()
        .with_network_policy(factory.network_policy().clone());
    let connector = WebSocketConnector::new(&factory)?;
    let helper = std::env::current_exe()?.with_file_name("codex-voice-capture");
    let mut command = Command::new(helper);
    if let Some(microphone) = microphone {
        command.arg(microphone);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .wrap_err("Could not start microphone helper; run npm run build:voice")?;
    let mut input = child
        .stdin
        .take()
        .ok_or_else(|| color_eyre::eyre::eyre!("Missing microphone control pipe"))?;
    let mut audio = BufReader::new(
        child
            .stdout
            .take()
            .ok_or_else(|| color_eyre::eyre::eyre!("Missing microphone audio pipe"))?,
    )
    .lines();
    let (utterances, mut pending) = mpsc::channel::<Vec<String>>(8);
    let transcription = async {
        while let Some(frames) = pending.recv().await {
            tokio::time::timeout(
                Duration::from_secs(45),
                transcribe(&connector, request.clone(), frames, emit),
            )
            .await
            .wrap_err("Timed out transcribing speech")??;
        }
        Ok::<(), color_eyre::eyre::Report>(())
    };
    tokio::pin!(transcription);
    let mut buffer = UtteranceBuffer::default();
    let mut microphone_ready = false;
    let startup_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        tokio::select! {
            result = &mut transcription => { return result; }
            _ = tokio::time::sleep_until(startup_deadline), if !microphone_ready => {
                bail!("Timed out waiting for microphone audio");
            }
            control = controls.recv() => {
                let Some(muted) = control else { return Ok(()); };
                input.write_all(if muted { b"mute\n" } else { b"unmute\n" }).await?;
            }
            line = audio.next_line() => {
                let Some(line) = line? else { bail!("Microphone helper exited"); };
                match serde_json::from_str::<CaptureEvent>(&line)? {
                    CaptureEvent::Ready => {
                        microphone_ready = true;
                        emit(DictationEvent::Ready);
                    }
                    CaptureEvent::Audio { audio, peak: value, rms, samples } => {
                        peak.fetch_max(value, Ordering::Relaxed);
                        if let Some(frames) = buffer.append(audio, rms, samples) {
                            utterances.try_send(frames).wrap_err("Dictation cannot keep up with microphone input")?;
                        }
                    }
                    CaptureEvent::Error { message } => { bail!("{message}"); },
                }
            }
        }
    }
}

async fn transcribe(
    connector: &WebSocketConnector,
    request: Request,
    frames: Vec<String>,
    emit: &(impl Fn(DictationEvent) + Sync),
) -> Result<()> {
    let (mut socket, _) = connector.connect(request, Default::default()).await?;
    socket
        .send(Message::Text(
            json!({
                "type": "session.start",
                "config": {
                    "input_audio_format": "pcm16", "sample_rate_hz": 16000, "num_channels": 1,
                    "max_buffer_size_bytes": 4194304, "max_utterance_duration_ms": 30000,
                    "session_ttl_ms": 60000, "provider_mode": "streaming_sse",
                    "transcript_delivery_mode": "final_only"
                }
            })
            .to_string()
            .into(),
        ))
        .await?;
    let mut started = false;
    while let Some(message) = socket.next().await {
        match message? {
            Message::Text(text) => {
                match serde_json::from_str::<ServerEvent>(&text)? {
                    ServerEvent::Started if !started => {
                        started = true;
                        for audio in &frames {
                            socket
                                .send(Message::Text(
                                    json!({"type": "audio.append", "audio": audio})
                                        .to_string()
                                        .into(),
                                ))
                                .await?;
                        }
                        // Dictation finalizes only after session.close, even after silence.
                        socket
                            .send(Message::Text(
                                json!({"type": "session.close"}).to_string().into(),
                            ))
                            .await?;
                    }
                    ServerEvent::Transcript { utterance_id, text } => {
                        let text = text.trim();
                        if !text.is_empty() {
                            emit(DictationEvent::Transcript {
                                utterance_id,
                                text: text.to_string(),
                            });
                        }
                    }
                    ServerEvent::Error { error } => {
                        bail!("{} ({})", error.message, error.code);
                    }
                    ServerEvent::Updated { session } if session.status == "closed" => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
            Message::Close(_) => {
                break;
            }
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await?,
            _ => {}
        }
    }
    bail!("Dictation connection closed before completion");
}

#[cfg(test)]
#[path = "dictation_tests.rs"]
mod realtime_tests;
