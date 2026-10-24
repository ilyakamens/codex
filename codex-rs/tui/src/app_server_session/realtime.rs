//! App-server signaling for TUI-owned realtime WebRTC sessions.

use super::AppServerSession;
use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::ThreadRealtimeAppendSpeechParams;
use codex_app_server_protocol::ThreadRealtimeAppendSpeechResponse;
use codex_app_server_protocol::ThreadRealtimeStartParams;
use codex_app_server_protocol::ThreadRealtimeStartResponse;
use codex_app_server_protocol::ThreadRealtimeStartTransport;
use codex_app_server_protocol::ThreadRealtimeStopParams;
use codex_app_server_protocol::ThreadRealtimeStopResponse;
use codex_protocol::ThreadId;
use codex_protocol::protocol::RealtimeConversationVersion;
use codex_protocol::protocol::RealtimeOutputModality;
use codex_protocol::protocol::RealtimeVoice;
use color_eyre::eyre::Result;
use color_eyre::eyre::WrapErr;

impl AppServerSession {
    pub(crate) async fn thread_realtime_start(
        &mut self,
        thread_id: ThreadId,
        offer_sdp: String,
        model: Option<String>,
        voice: Option<RealtimeVoice>,
        text_output: bool,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStartResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStart {
                request_id,
                params: ThreadRealtimeStartParams {
                    thread_id: thread_id.to_string(),
                    client_managed_handoffs: Some(true),
                    delegation_ack_filler: text_output.then_some(false),
                    flush_transcript_tail_on_session_end: None,
                    codex_responses_as_items: None,
                    codex_response_item_prefix: None,
                    codex_response_handoff_mode: None,
                    backend_reasoning_status: false,
                    codex_response_handoff_channel_prefixes: None,
                    model,
                    output_modality: RealtimeOutputModality::Audio,
                    include_startup_context: Some(false),
                    initial_items: None,
                    realtime_start_instructions: text_output.then(|| {
                        "The user is speaking and reads your responses directly in the terminal. \
                         Treat realtime delegation input as the user's request and respond normally, \
                         including progress updates, Markdown, and code when appropriate. \
                         Your responses are displayed verbatim, not spoken or summarized by an intermediary."
                            .to_string()
                    }),
                    realtime_end_instructions: None,
                    prompt: text_output.then(|| {
                        Some("You provide voice input for Codex. Delegate every user request to the backend, \
                         including questions and conversation, preserving the user's words. \
                         The user reads the backend's replies directly in the terminal. \
                         Do not answer requests yourself or speak acknowledgments or results."
                            .to_string())
                    }),
                    realtime_session_id: None,
                    transport: Some(ThreadRealtimeStartTransport::Webrtc { sdp: offer_sdp }),
                    version: Some(RealtimeConversationVersion::V3),
                    voice,
                },
            })
            .await
            .wrap_err("thread/realtime/start failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_stop(&mut self, thread_id: ThreadId) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeStopResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeStop {
                request_id,
                params: ThreadRealtimeStopParams {
                    thread_id: thread_id.to_string(),
                },
            })
            .await
            .wrap_err("thread/realtime/stop failed in TUI")?;
        Ok(())
    }

    pub(crate) async fn thread_realtime_append_speech(
        &mut self,
        thread_id: ThreadId,
        text: String,
    ) -> Result<()> {
        let request_id = self.next_request_id();
        let _: ThreadRealtimeAppendSpeechResponse = self
            .client
            .request_typed(ClientRequest::ThreadRealtimeAppendSpeech {
                request_id,
                params: ThreadRealtimeAppendSpeechParams {
                    thread_id: thread_id.to_string(),
                    text,
                },
            })
            .await
            .wrap_err("thread/realtime/appendSpeech failed in TUI")?;
        Ok(())
    }
}
