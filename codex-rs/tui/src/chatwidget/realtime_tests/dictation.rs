use super::*;
use crate::dictation::DictationEvent;
use pretty_assertions::assert_eq;

fn transcript(id: &str, text: &str) -> DictationEvent {
    DictationEvent::Transcript {
        utterance_id: id.into(),
        text: text.into(),
    }
}

#[tokio::test]
async fn dictation_submits_one_plain_user_turn_and_preserves_the_draft() {
    let (mut chat, _sender, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.bottom_pane
        .set_composer_text("Unfinished draft".into(), Vec::new(), Vec::new());

    chat.on_dictation_event(
        0,
        transcript("utterance-1", "Run Python to calculate 17 plus 26."),
    );
    chat.on_dictation_event(
        0,
        transcript("utterance-1", "Run Python to calculate 17 plus 26."),
    );

    let AppCommand::UserTurn { items, .. } = ops.try_recv().expect("normal user turn") else {
        panic!("dictation must use the normal user turn, without a realtime handoff");
    };
    assert_eq!(
        items,
        vec![UserInput::Text {
            text: "Run Python to calculate 17 plus 26.".into(),
            text_elements: Vec::new(),
        }]
    );
    assert!(ops.try_recv().is_err());
    assert_eq!(chat.composer_text_with_pending(), "Unfinished draft");
}

#[tokio::test]
async fn dictation_does_not_run_shell_shortcuts() {
    let (mut chat, _sender, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.on_dictation_event(0, transcript("utterance-1", "!say hello"));
    assert!(
        matches!(ops.try_recv(), Ok(AppCommand::UserTurn { items, .. })
        if matches!(&items[..], [UserInput::Text { text, .. }] if text == "!say hello"))
    );
}

#[tokio::test]
async fn dictation_accepts_the_last_utterance_after_mute_but_not_after_stop() {
    let (mut chat, _sender, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.realtime_conversation.microphone_muted = true;
    chat.on_dictation_event(0, transcript("utterance-1", "Finish this request."));
    assert!(matches!(ops.try_recv(), Ok(AppCommand::UserTurn { .. })));
    chat.reset_realtime_conversation();
    chat.on_dictation_event(0, transcript("utterance-2", "Too late."));
    assert!(ops.try_recv().is_err());
}

#[tokio::test]
async fn dictation_rejects_events_from_an_old_session() {
    let (mut chat, _sender, _events, mut ops) = make_chatwidget_manual_with_sender().await;
    activate_voice(&mut chat);
    chat.realtime_conversation.attempt_id = 2;
    chat.on_dictation_event(1, transcript("utterance-1", "Old request."));
    chat.on_dictation_event(1, DictationEvent::Error("Old error".into()));
    assert!(ops.try_recv().is_err());
    chat.on_dictation_event(2, transcript("utterance-1", "Current request."));
    assert!(matches!(ops.try_recv(), Ok(AppCommand::UserTurn { .. })));
}
