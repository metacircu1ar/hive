//! Approval cards regressions.
use super::*;

#[test]
fn approvals_answer_from_the_card_and_forms_validate_first() {
    let mut app = app("idle", false);
    app.update(Update::Approval(
        json!({"id": "q", "state": "pending", "message": "Go?"}),
    ));
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(
        writes(&mut app)[0].2,
        json!({"op": "respond", "approval_id": "q", "action": "accept"})
    );
    app.update(Update::Approval(
        json!({"id": "f", "state": "pending", "message": "Region?",
        "schema": {"type": "object", "required": ["region"],
                   "properties": {"region": {"type": "string", "enum": ["eu", "us"]}}}}),
    ));
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty() && app.status.contains("region is required"));
    press(&mut app, KeyCode::Right, KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["content"], json!({"region": "eu"}));
    // From the input, /decline answers the same question.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    typed(&mut app, "/decline");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["action"], json!("decline"));
    // Once answered, the card closes and focus returns to the input.
    app.update(Update::Approval(Value::Null));
    assert!(
        app.screen(true, Rect::new(0, 0, 80, 24))
            .unwrap()
            .card
            .is_none()
    );
}

#[test]
fn form_cards_ignore_single_answer_keys_and_take_pasted_text() {
    let mut app = app("idle", false);
    app.update(Update::Approval(json!({"id": "f", "state": "pending", "message": "Name?",
        "schema": {"type": "object", "required": ["name"],
                   "properties": {"name": {"type": "string"}, "mode": {"type": "string", "enum": ["a", "b"]}}}})));
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    app.event(&Event::Paste(String::from("release")));
    // Typed letters go to the text field rather than answering the form.
    press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('d'), KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        writes(&mut app)[0].2["content"],
        json!({"name": "released"})
    );
}

#[test]
fn hosts_set_the_hint_track_writes_and_focus_the_next_card() {
    let mut app = App::new(String::from("a"), Mode::default());
    app.set_hint(String::from("custom keys"));
    app.update(snapshot("idle", false));
    assert_eq!(app.status, "custom keys");
    app.set_hint(String::from("newer keys"));
    assert_eq!(app.status, "newer keys");
    assert!(!app.busy());
    typed(&mut app, "reply: x");
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert!(app.busy());
    let action = writes(&mut app)[0].0;
    app.update(Update::Done {
        action,
        label: String::from("send"),
        result: Ok(views::read(&json!({}))),
    });
    assert!(!app.busy());
    // Asked before the card exists, focus moves once it arrives.
    app.focus_card();
    assert_eq!(app.focus, Focus::Input);
    app.update(Update::Approval(json!({"id": "q1", "kind": "approval",
        "request": {"message": "Deploy?"}})));
    assert_eq!(app.focus, Focus::Card);
    app.focus = Focus::Input;
    app.update(Update::Approval(json!({"id": "q2", "kind": "approval",
        "request": {"message": "Again?"}})));
    assert_eq!(app.focus, Focus::Input);
    app.focus_card();
    assert_eq!(app.focus, Focus::Card);
}

#[test]
fn card_buttons_answer_when_clicked() {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};
    let left = MouseEventKind::Down(MouseButton::Left);
    for (label, action) in [
        (" Accept ", "accept"),
        (" Decline ", "decline"),
        (" Cancel ", "cancel"),
    ] {
        let mut app = app("idle", false);
        app.update(Update::Approval(json!({"id": "q1", "message": "Deploy?"})));
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal
            .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 24), true))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let at = (0..24)
            .find_map(|y| {
                let row: String = (0..60)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect();
                row.find(label)
                    .map(|byte| (u16::try_from(row[..byte].chars().count()).unwrap() + 1, y))
            })
            .unwrap();
        assert!(mouse_at(&mut app, left, at));
        let written = writes(&mut app);
        assert_eq!(written.len(), 1, "{label}");
        assert_eq!(
            (&written[0].2["op"], &written[0].2["action"]),
            (&json!("respond"), &json!(action))
        );
    }
}
