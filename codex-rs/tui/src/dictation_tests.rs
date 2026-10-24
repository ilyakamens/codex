use super::*;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use pretty_assertions::assert_eq;
use std::sync::Mutex;

#[test]
fn silence_does_not_submit_and_a_pause_finishes_each_utterance_once() {
    let mut buffer = UtteranceBuffer::default();
    for _ in 0..30 {
        assert!(buffer.append("silence".into(), 0, 1600).is_none());
    }
    for speech in ["first request", "second request"] {
        assert!(buffer.append(speech.into(), 500, 1600).is_none());
        for _ in 0..11 {
            assert!(buffer.append("silence".into(), 0, 1600).is_none());
        }
        let audio = buffer.append("silence".into(), 0, 1600).unwrap();
        assert_eq!(audio.iter().filter(|frame| *frame == speech).count(), 1);
        assert!(
            audio
                .iter()
                .all(|frame| frame == speech || frame == "silence")
        );
        for _ in 0..20 {
            assert!(buffer.append("silence".into(), 0, 1600).is_none());
        }
    }
}

#[test]
fn short_pause_keeps_the_sentence_and_its_quiet_prefix_together() {
    let mut buffer = UtteranceBuffer::default();
    assert!(buffer.append("quiet beginning".into(), 50, 1600).is_none());
    assert!(buffer.append("first words".into(), 500, 1600).is_none());
    assert!(buffer.append("short pause".into(), 0, 8000).is_none());
    assert!(buffer.append("last words".into(), 500, 1600).is_none());
    let audio = buffer.append("end pause".into(), 0, 19200).unwrap();
    assert_eq!(
        audio,
        [
            "quiet beginning",
            "first words",
            "short pause",
            "last words",
            "end pause"
        ]
    );
}

#[tokio::test]
async fn completed_audio_is_finalized_before_submitting_the_transcript() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let request = format!("ws://{}", listener.local_addr()?).into_client_request()?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        let start: serde_json::Value =
            serde_json::from_str(socket.next().await.unwrap()?.to_text()?)?;
        assert_eq!(start["type"], "session.start");
        socket
            .send(Message::Text(
                json!({"type":"session.started"}).to_string().into(),
            ))
            .await?;
        for audio in ["first audio", "last audio"] {
            let frame: serde_json::Value =
                serde_json::from_str(socket.next().await.unwrap()?.to_text()?)?;
            assert_eq!(frame, json!({"type":"audio.append", "audio":audio}));
        }
        let finish: serde_json::Value =
            serde_json::from_str(socket.next().await.unwrap()?.to_text()?)?;
        assert_eq!(finish["type"], "session.close");
        for event in [
            json!({"type":"transcript.final", "utterance_id":"utterance-1", "text":" Calculate 17 plus 26. "}),
            json!({"type":"session.updated", "session":{"status":"closed"}}),
        ] {
            socket.send(Message::Text(event.to_string().into())).await?;
        }
        Ok::<(), color_eyre::eyre::Report>(())
    });
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault);
    let connector = WebSocketConnector::new(&factory)?;
    let events = Mutex::new(Vec::new());
    tokio::time::timeout(
        Duration::from_secs(5),
        transcribe(
            &connector,
            request,
            vec!["first audio".into(), "last audio".into()],
            &|event| events.lock().unwrap().push(event),
        ),
    )
    .await??;
    server.await??;
    let events = events.into_inner().unwrap();
    assert!(
        matches!(&events[..], [DictationEvent::Transcript { utterance_id, text }]
        if utterance_id == "utterance-1" && text == "Calculate 17 plus 26.")
    );
    Ok(())
}

#[tokio::test]
async fn silence_completed_without_a_transcript_keeps_dictation_available() -> Result<()> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let request = format!("ws://{}", listener.local_addr()?).into_client_request()?;
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await?;
        let mut socket = tokio_tungstenite::accept_async(stream).await?;
        socket.next().await.unwrap()?;
        socket
            .send(Message::Text(
                json!({"type":"session.started"}).to_string().into(),
            ))
            .await?;
        loop {
            let message: serde_json::Value =
                serde_json::from_str(socket.next().await.unwrap()?.to_text()?)?;
            if message["type"] == "session.close" {
                break;
            }
        }
        socket
            .send(Message::Text(
                json!({"type":"session.updated", "session":{"status":"closed"}})
                    .to_string()
                    .into(),
            ))
            .await?;
        Ok::<(), color_eyre::eyre::Report>(())
    });
    let connector =
        WebSocketConnector::new(&HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault))?;
    tokio::time::timeout(
        Duration::from_secs(5),
        transcribe(&connector, request, vec!["noise".into()], &|_| {
            panic!("no input should be submitted")
        }),
    )
    .await??;
    server.await??;
    Ok(())
}
