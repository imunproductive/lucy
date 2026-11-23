//! The bot only talks when the player turns "Bot talks" on in the dashboard
//! (`ow174/lobby/handlers/chat.py` chat and whisper, `BOT_REPLY`). Ports
//! `tests/test_bot_chat.py`.

use lucy_lobby::accounts::Profile;

use super::*;

/// The texts of the chat lines the player gets after saying "hi" in a
/// channel the bot is in (General).
fn channel_texts(alpha: &mut Client<'_>) -> Vec<String> {
    alpha.send(
        CHAT_OUT,
        21700,
        Fields::from([
            (0x78, Value::Struct(general_channel())),
            (0x90, Value::String("hi".into())),
        ]),
    );
    alpha
        .take()
        .into_iter()
        .filter(|&(crc, id, _)| (crc, id) == (CHAT_IN, 20400))
        .map(|(_, _, line)| match &line[&0x100] {
            Value::String(text) => text.clone(),
            _ => panic!("a chat line without text"),
        })
        .collect()
}

/// The whispers (27117) the player gets back after whispering "hi" to the
/// bot.
fn whisper_replies(alpha: &mut Client<'_>) -> Vec<Fields> {
    alpha.send(
        FRIENDS_OUT,
        27004,
        Fields::from([
            (0x78, Value::Unsigned(9)),
            (0x80, id("Bot")),
            (0xa0, Value::String("hi".into())),
        ]),
    );
    alpha
        .take()
        .into_iter()
        .filter(|&(crc, id, _)| (crc, id) == (FRIENDS, 27117))
        .map(|(_, _, whisper)| whisper)
        .collect()
}

/// Alpha logged in and in General, with nothing left to read.
fn logged_in<'a>(schemas: &'a Schemas, server: &Server) -> Client<'a> {
    let mut alpha = login(schemas, SocketAddr::from(server.lobby_addr()), "Alpha");
    alpha.expect(CHAT_IN, 20402);
    alpha.settle();
    alpha
}

/// `tests/test_bot_chat.py::BotChatTests`: `test_the_bot_is_quiet_by_default`,
/// then `test_the_bot_answers_when_the_switch_is_on`.
#[test]
#[ignore = "requires an authorized 104319 runtime in LUCY_RUNTIME_DIR"]
fn the_bot_answers_only_when_the_switch_is_on() {
    assert!(!Profile::default().bot_chat);
    let state = tempfile::tempdir().unwrap();
    let (mut server, time) = start(&state);
    let dashboard = server.dashboard_addr().unwrap();
    let schemas = Schemas::lobby(&lucy_testkit::runtime_dir()).unwrap();
    let mut alpha = logged_in(&schemas, &server);

    assert_eq!(channel_texts(&mut alpha), ["hi"]);
    assert!(whisper_replies(&mut alpha).is_empty());

    let reply = post(
        dashboard,
        "/api/update_profile",
        r#"{"account":"Alpha","bot_chat":true}"#,
    );
    assert!(
        reply.starts_with("HTTP/1.0 200") || reply.starts_with("HTTP/1.1 200"),
        "{reply}"
    );
    // The edit reaches the game at the session's next tick (`services.rs`).
    run(&time.lobby, &mut [&mut alpha], TICK);
    alpha.settle();

    let texts = channel_texts(&mut alpha);
    assert_eq!((texts.len(), texts[0].as_str()), (2, "hi"), "{texts:?}");
    assert_eq!(whisper_replies(&mut alpha).len(), 1);

    alpha.close();
    server.stop().unwrap();
}
