//! Selection pickers regressions.
use super::*;

#[test]
fn model_and_effort_without_an_argument_pick_from_the_listed_names() {
    let mut app = listed_app(&["gpt-6", "m", "gpt-6-luna"]);
    run(&mut app, "/model");
    assert_eq!(app.focus, Focus::Card);
    assert!(app.input.is_empty());
    let (title, text) = card_text(&app).unwrap();
    assert_eq!(title, "model (codex)");
    // It starts on the agent's current model.
    assert_eq!(text, ["   gpt-6", "▸✓ m", "   gpt-6-luna"]);
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(
        writes(&mut app)[0].2,
        json!({"op": "set_options", "target": "a", "options": {"model": "gpt-6-luna"}})
    );
    assert_eq!(app.focus, Focus::Input);
    assert!(card_text(&app).is_none());
    run(&mut app, "/effort");
    assert_eq!(card_text(&app).unwrap().0, "effort (codex)");
    // Chords such as the TUI's prefix never move or pick.
    press(&mut app, KeyCode::Down, KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Enter, KeyModifiers::ALT);
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    press(&mut app, KeyCode::Enter, KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["options"], json!({"effort": "high"}));
    // Esc closes without a write; a typed name still sets any value.
    run(&mut app, "/effort");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty() && card_text(&app).is_none());
    assert_eq!(app.focus, Focus::Input);
    run(&mut app, "/model custom-1");
    assert_eq!(
        writes(&mut app)[0].2["options"],
        json!({"model": "custom-1"})
    );
}

#[test]
fn a_click_sets_the_name_under_it_in_a_scrolled_picker() {
    use ratatui::{Terminal, backend::TestBackend};
    let names: Vec<String> = (0..20).map(|index| format!("model-{index:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut app = listed_app(&refs);
    run(&mut app, "/model");
    for _ in 0..15 {
        press(&mut app, KeyCode::Down, KeyModifiers::NONE);
    }
    let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
    terminal
        .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 24), true))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let row = |label: &str| {
        (0..24).find(|y| {
            (0..60)
                .map(|x| buffer[(x, *y)].symbol().to_owned())
                .collect::<String>()
                .contains(label)
        })
    };
    // Twelve text rows: names 4 to 15 under the frame title.
    assert!(row("model (codex)").is_some());
    assert!(row("model-03").is_none() && row("model-04").is_some() && row("model-15").is_some());
    let left = MouseEventKind::Down(MouseButton::Left);
    // The footer row and the border pick nothing.
    let footer = row("Enter set").unwrap();
    assert!(mouse_at(&mut app, left, (4, footer)));
    assert!(mouse_at(&mut app, left, (4, footer + 1)));
    assert!(writes(&mut app).is_empty());
    assert!(mouse_at(&mut app, left, (4, row("model-07").unwrap())));
    assert_eq!(
        writes(&mut app)[0].2["options"],
        json!({"model": "model-07"})
    );
    assert!(card_text(&app).is_none());
}

#[test]
fn footer_rows_esc_in_the_input_and_attention_jumps_close_or_ignore_the_picker() {
    use ratatui::{Terminal, backend::TestBackend};
    let names: Vec<String> = (0..20).map(|index| format!("model-{index:02}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    // A short view squeezes the card: its footer covers a name row.
    let mut app = listed_app(&refs);
    run(&mut app, "/model");
    let mut terminal = Terminal::new(TestBackend::new(60, 14)).unwrap();
    terminal
        .draw(|frame| app.draw_into(frame, Rect::new(0, 0, 60, 14), true))
        .unwrap();
    let card = app.shown.regions.card;
    // Twelve rows were asked for: nine names, the footer and the border.
    assert!(card.height < 12, "{card:?}");
    let footer = card.y + card.height - 2;
    let left = MouseEventKind::Down(MouseButton::Left);
    assert!(mouse_at(&mut app, left, (4, footer)));
    assert!(writes(&mut app).is_empty());
    assert!(card_text(&app).is_some());
    // With a question under the picker and the input focused, Esc closes the
    // picker and a second Esc does not interrupt.
    app.update(Update::Approval(json!({"id": "q", "message": "Go?"})));
    app.focus = Focus::Input;
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert_eq!(card_text(&app).unwrap().0, "question");
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    assert!(writes(&mut app).is_empty());
    // An attention jump asked for before the question arrives closes a
    // picker opened meanwhile.
    let mut app = listed_app(&refs);
    app.focus_card();
    run(&mut app, "/model");
    app.update(Update::Approval(json!({"id": "r", "message": "Ship?"})));
    assert_eq!(app.focus, Focus::Card);
    assert_eq!(card_text(&app).unwrap().0, "question");
}

#[test]
fn the_picker_covers_a_waiting_question_until_it_closes() {
    let mut app = listed_app(&["gpt-6", "m"]);
    app.update(Update::Approval(json!({"id": "q", "message": "Go?"})));
    run(&mut app, "/model");
    assert_eq!(card_text(&app).unwrap().0, "model (codex)");
    assert!(
        app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    // Answer keys and pastes do nothing while the picker has the card's place.
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    app.event(&Event::Paste(String::from("eu")));
    assert!(writes(&mut app).is_empty());
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    let (title, text) = card_text(&app).unwrap();
    assert_eq!(title, "question");
    // The note about the covered question goes with the picker.
    assert!(
        !app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    assert!(text.iter().any(|line| line == "Go?"), "{text:?}");
    press(&mut app, KeyCode::Tab, KeyModifiers::NONE);
    assert_eq!(app.focus, Focus::Card);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::NONE);
    assert_eq!(writes(&mut app)[0].2["action"], json!("accept"));
    // A question answered elsewhere leaves an open picker in place.
    press(&mut app, KeyCode::Esc, KeyModifiers::NONE);
    run(&mut app, "/model");
    app.update(Update::Approval(json!({"id": "q2", "message": "Again?"})));
    assert!(
        app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    app.update(Update::Approval(Value::Null));
    assert_eq!(app.focus, Focus::Card);
    assert_eq!(card_text(&app).unwrap().0, "model (codex)");
    assert!(
        !app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    // A question arriving under the picker is announced; the attention
    // list's jump to it closes the picker.
    app.status.clear();
    app.update(Update::Approval(json!({"id": "r", "message": "Ship?"})));
    assert_eq!(card_text(&app).unwrap().0, "model (codex)");
    assert!(
        app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
    app.focus_card();
    assert_eq!(app.focus, Focus::Card);
    assert_eq!(card_text(&app).unwrap().0, "question");
    assert!(
        !app.status.contains("a question is waiting"),
        "{}",
        app.status
    );
}
