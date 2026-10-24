use super::*;
use codex_features::Feature;
use pretty_assertions::assert_eq;

fn take_history(events: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> String {
    std::iter::from_fn(|| events.try_recv().ok())
        .filter_map(|event| match event {
            AppEvent::InsertHistoryCell(cell) => Some(
                cell.transcript_lines(/*width*/ 80)
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn voice_text_output_streams_updates_and_final_answer_without_speech() {
    let (mut chat, _sender, mut events, mut ops) = make_chatwidget_manual_with_sender().await;
    chat.config
        .features
        .enable(Feature::VoiceTextOutput)
        .unwrap();
    let thread_id = activate_voice(&mut chat);
    let turn_id = "voice-text-turn";
    chat.turn_lifecycle.last_turn_id = Some(turn_id.into());
    chat.on_realtime_transcript_done("user".into(), "show the code".into());
    commit_realtime_history_events(&mut chat, &mut events);
    let mut rendered = take_history(&mut events);
    let delegation =
        user_item("<realtime_delegation><input>show the code</input></realtime_delegation>");
    start_item(&mut chat, thread_id, turn_id, delegation.clone());
    complete_item(&mut chat, thread_id, turn_id, delegation);
    let commentary = agent_item(
        "update",
        "Checking the code.",
        Some(MessagePhase::Commentary),
    );
    start_item(&mut chat, thread_id, turn_id, commentary.clone());
    chat.handle_server_notification(
        ServerNotification::AgentMessageDelta(AgentMessageDeltaNotification {
            thread_id: thread_id.to_string(),
            turn_id: turn_id.into(),
            item_id: "update".into(),
            delta: "Checking the code.\n".into(),
        }),
        None,
    );
    chat.on_commit_tick();
    rendered.push_str(&take_history(&mut events));
    assert!(rendered.contains("Checking the code."), "{rendered}");
    complete_item(&mut chat, thread_id, turn_id, commentary);

    let answer = agent_item(
        "answer",
        "Here is the code:\n\n```rust\nlet answer = 42;\n```",
        Some(MessagePhase::FinalAnswer),
    );
    start_item(&mut chat, thread_id, turn_id, answer.clone());
    complete_item(&mut chat, thread_id, turn_id, answer.clone());
    finish_turn(
        &mut chat,
        thread_id,
        turn_id,
        vec![answer],
        TurnStatus::Completed,
    );
    chat.flush_answer_stream_with_separator();
    commit_realtime_history_events(&mut chat, &mut events);
    rendered.push_str(&take_history(&mut events));
    chat.on_realtime_conversation_closed(Some("requested".into()));
    commit_realtime_history_events(&mut chat, &mut events);
    rendered.push_str(&take_history(&mut events));

    assert_eq!(rendered.matches("show the code").count(), 1, "{rendered}");
    assert_eq!(
        rendered.matches("let answer = 42;").count(),
        1,
        "{rendered}"
    );
    assert!(!rendered.contains("<realtime_delegation>"), "{rendered}");
    assert!(matches!(
        chat.pending_notification,
        Some(crate::chatwidget::Notification::AgentTurnComplete { .. })
    ));
    assert!(
        ops.try_recv().is_err(),
        "text replies must not request speech"
    );
}

#[tokio::test]
async fn voice_text_output_keeps_user_captions_and_hides_voice_service_replies() {
    let (mut chat, _sender, mut events, _ops) = make_chatwidget_manual_with_sender().await;
    chat.config
        .features
        .enable(Feature::VoiceTextOutput)
        .unwrap();
    activate_voice(&mut chat);
    chat.on_realtime_transcript_delta("user".into(), "Spoken request".into());
    chat.on_realtime_transcript_done("user".into(), "Spoken request".into());
    chat.on_realtime_transcript_delta("assistant".into(), "Voice acknowledgment".into());
    chat.on_realtime_transcript_done("assistant".into(), "Voice acknowledgment".into());
    commit_realtime_history_events(&mut chat, &mut events);
    let rendered = take_history(&mut events);
    assert_eq!(rendered.matches("Spoken request").count(), 1);
    assert!(!rendered.contains("Voice acknowledgment"));
    assert!(chat.realtime_microphone_is_listening());
}

#[tokio::test]
async fn voice_text_output_replays_normal_updates_and_answers() {
    let (mut chat, _sender, mut events, mut ops) = make_chatwidget_manual_with_sender().await;
    chat.config
        .features
        .enable(Feature::VoiceTextOutput)
        .unwrap();
    chat.thread_id = Some(ThreadId::new());
    chat.replay_thread_turns(
        vec![Turn {
            id: "saved-voice-turn".into(),
            root_turn_id: None,
            items: vec![
                user_item("<realtime_delegation><input>question</input></realtime_delegation>"),
                agent_item(
                    "update",
                    "Checking the code.",
                    Some(MessagePhase::Commentary),
                ),
                agent_item(
                    "answer",
                    "The answer is 42.",
                    Some(MessagePhase::FinalAnswer),
                ),
            ],
            items_view: TurnItemsView::Full,
            status: TurnStatus::Completed,
            error: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
        }],
        ReplayKind::ThreadSnapshot,
    );
    chat.flush_answer_stream_with_separator();
    let rendered = take_history(&mut events);
    assert!(rendered.contains("Checking the code."), "{rendered}");
    assert_eq!(
        rendered.matches("The answer is 42.").count(),
        1,
        "{rendered}"
    );
    assert!(!rendered.contains("<realtime_delegation>"));
    assert!(ops.try_recv().is_err());
}
