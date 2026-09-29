//! Сквозная проверка доставки: два независимых клиента через настоящий Node.
//!
//! Без флага `testing` файл превращается в пустой: обычная сборка не должна зависеть
//! от тестовой обёртки над ядром.
#![cfg(feature = "testing")]

//!
//! Тест намеренно не запускается вместе с остальными — ему нужен живой узел:
//! `TURAT_TEST_NODE=http://localhost:18080 cargo test --test delivery -- --ignored`

use std::path::PathBuf;

use serde_json::{Value, json};
use turattext_core::testing::TestCore;

fn node_url() -> Option<String> {
    std::env::var("TURAT_TEST_NODE").ok()
}

struct Client {
    core: TestCore,
    user_id: String,
}

impl Client {
    fn open(name: &str, node: &str) -> Self {
        let root = std::env::temp_dir().join(format!("turat-e2e-{name}-{}", uuid::Uuid::new_v4()));
        let mut core = TestCore::open(&root);
        let connected = core.call(&json!({"command": "connect", "bootstrap_url": node}));
        assert_eq!(connected["ok"], true, "подключение к Node: {connected}");
        let saved = core.call(&json!({
            "command": "save_profile",
            "username": "",
            "display_name": name,
            "about": "",
            "avatar_base64": Value::Null,
        }));
        assert_eq!(saved["ok"], true, "{saved}");
        let user_id = saved["snapshot"]["identity"]["userId"]
            .as_str()
            .expect("userId")
            .to_owned();
        Self { core, user_id }
    }

    fn sync(&mut self) -> Value {
        self.core.call(&json!({"command": "sync"}))
    }

    fn add(&mut self, peer: &str) {
        let added = self.core.call(&json!({
            "command": "add_contact",
            "query": peer,
            "display_name": "Собеседник",
        }));
        assert_eq!(added["ok"], true, "{added}");
    }

    fn send(&mut self, peer: &str, text: &str) -> Value {
        self.core.call(&json!({
            "command": "send_text",
            "user_id": peer,
            "text": text,
        }))
    }

    fn conversation(&mut self, peer: &str) -> Vec<Value> {
        let snapshot = self
            .core
            .call(&json!({"command": "select_contact", "user_id": peer}));
        snapshot["snapshot"]["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }
}

/// Сообщение проходит весь путь: очередь, ящик на Node, храповик и история собеседника.
#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_message_reaches_the_other_side() {
    let Some(node) = node_url() else {
        panic!("Задайте TURAT_TEST_NODE");
    };
    let mut alice = Client::open("Алиса", &node);
    let mut bob = Client::open("Боб", &node);

    // Оба публикуют адрес и предключи — без этого писать друг другу некуда.
    assert_eq!(alice.sync()["ok"], true);
    assert_eq!(bob.sync()["ok"], true);

    let bob_id = bob.user_id.clone();
    let alice_id = alice.user_id.clone();
    alice.add(&bob_id);
    let sent = alice.send(&bob_id, "Привет с другого устройства");
    assert_eq!(sent["ok"], true, "{sent}");

    // Боб забирает конверт: контакт появляется как запрос, сообщение — в истории.
    assert_eq!(bob.sync()["ok"], true);
    let received = bob.conversation(&alice_id);
    assert_eq!(received.len(), 1, "у Боба нет сообщения: {received:?}");
    assert_eq!(received[0]["text"], "Привет с другого устройства");
    assert_eq!(received[0]["outgoing"], false);

    // Боб принимает запрос и отвечает — теперь работает личный ящик, а не публичный.
    let accepted = bob
        .core
        .call(&json!({"command": "accept_contact", "user_id": alice_id}));
    assert_eq!(accepted["ok"], true, "{accepted}");
    let replied = bob.send(&alice_id, "И тебе привет");
    assert_eq!(replied["ok"], true, "{replied}");

    assert_eq!(alice.sync()["ok"], true);
    let dialogue = alice.conversation(&bob_id);
    assert_eq!(dialogue.len(), 2, "у Алисы нет ответа: {dialogue:?}");
    assert_eq!(dialogue[1]["text"], "И тебе привет");

    // Квитанция о доставке возвращается тем же путём и закрывает первую галочку.
    assert_eq!(bob.sync()["ok"], true);
    assert_eq!(alice.sync()["ok"], true);
    let dialogue = alice.conversation(&bob_id);
    assert_eq!(dialogue[0]["delivered"], true, "нет квитанции: {dialogue:?}");
}

/// Правка, реакция и удаление доезжают до собеседника и меняют уже показанное сообщение.
#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn edits_and_reactions_follow_the_message() {
    let Some(node) = node_url() else {
        panic!("Задайте TURAT_TEST_NODE");
    };
    let mut alice = Client::open("Алиса", &node);
    let mut bob = Client::open("Боб", &node);
    assert_eq!(alice.sync()["ok"], true);
    assert_eq!(bob.sync()["ok"], true);
    let bob_id = bob.user_id.clone();
    let alice_id = alice.user_id.clone();

    alice.add(&bob_id);
    assert_eq!(alice.send(&bob_id, "первая версия")["ok"], true);
    assert_eq!(bob.sync()["ok"], true);
    assert_eq!(
        bob.core
            .call(&json!({"command": "accept_contact", "user_id": alice_id}))["ok"],
        true
    );

    // Алиса забирает подтверждение приёма: вместе с ним приходит личный обратный
    // адрес Боба, без которого правки и реакции в его ящик не пройдут.
    assert_eq!(alice.sync()["ok"], true);

    let event_id = alice.conversation(&bob_id)[0]["eventId"]
        .as_str()
        .expect("eventId")
        .to_owned();
    assert_eq!(
        alice.core.call(&json!({
            "command": "edit_message",
            "event_id": event_id,
            "text": "исправленная версия",
        }))["ok"],
        true
    );
    assert_eq!(bob.sync()["ok"], true);
    assert_eq!(bob.conversation(&alice_id)[0]["text"], "исправленная версия");

    assert_eq!(
        bob.core.call(&json!({
            "command": "react",
            "event_ids": [event_id],
            "reaction": "🔥",
        }))["ok"],
        true
    );
    assert_eq!(alice.sync()["ok"], true);
    assert_eq!(alice.conversation(&bob_id)[0]["reactions"][0], "🔥");

    assert_eq!(
        alice
            .core
            .call(&json!({"command": "delete_messages", "event_ids": [event_id]}))["ok"],
        true
    );
    assert_eq!(bob.sync()["ok"], true);
    assert_eq!(bob.conversation(&alice_id)[0]["deleted"], true);
}

fn call_ok(client: &mut Client, command: Value) -> Value {
    let response = client.core.call(&command);
    assert_eq!(response["ok"], true, "{command}: {response}");
    response
}

fn texts(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|message| message["service"] != true)
        .map(|message| message["text"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// Группа из трёх человек, где двое участников друг с другом не знакомы: приглашение,
/// вступление, переписка, назначение администратора и исключение.
#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_group_reaches_every_member_and_forgets_the_removed() {
    let Some(node) = node_url() else {
        panic!("Задайте TURAT_TEST_NODE");
    };
    let mut alice = Client::open("Алиса", &node);
    let mut bob = Client::open("Боб", &node);
    let mut carol = Client::open("Кэрол", &node);
    for client in [&mut alice, &mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
    }
    let (alice_id, bob_id, carol_id) = (alice.user_id.clone(), bob.user_id.clone(), carol.user_id.clone());

    // Алиса знакома с обоими, Боб и Кэрол — нет.
    for (peer, peer_id) in [(&mut bob, bob_id.clone()), (&mut carol, carol_id.clone())] {
        alice.add(&peer_id);
        assert_eq!(alice.send(&peer_id, "привет")["ok"], true);
        assert_eq!(peer.sync()["ok"], true);
        call_ok(peer, json!({"command": "accept_contact", "user_id": alice_id}));
    }
    assert_eq!(alice.sync()["ok"], true);

    let created = call_ok(&mut alice, json!({
        "command": "create_group",
        "name": "Тройка",
        "member_ids": [bob_id, carol_id],
    }));
    let group_id = created["value"]["groupId"].as_str().unwrap().to_owned();
    assert_eq!(alice.sync()["ok"], true);

    for client in [&mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
        let invited = client.conversation(&group_id);
        assert!(!invited.is_empty(), "приглашение не дошло");
        let snapshot = call_ok(client, json!({"command": "select_contact", "user_id": group_id}));
        assert_eq!(snapshot["snapshot"]["group"]["pendingInvite"], true);
        call_ok(client, json!({"command": "accept_contact", "user_id": group_id}));
    }
    // Вступление Боба доходит до незнакомой ему Кэрол через её публичный ящик,
    // ответ несёт её личный адрес обратно.
    for _ in 0..2 {
        for client in [&mut alice, &mut bob, &mut carol] {
            assert_eq!(client.sync()["ok"], true);
        }
    }

    call_ok(&mut alice, json!({"command": "send_text", "user_id": group_id, "text": "всем от Алисы"}));
    call_ok(&mut bob, json!({"command": "send_text", "user_id": group_id, "text": "всем от Боба"}));
    for client in [&mut alice, &mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
    }
    let seen_by_carol = texts(&carol.conversation(&group_id));
    assert!(seen_by_carol.contains(&"всем от Алисы".to_owned()), "{seen_by_carol:?}");
    assert!(seen_by_carol.contains(&"всем от Боба".to_owned()), "{seen_by_carol:?}");
    let bob_message = carol
        .conversation(&group_id)
        .into_iter()
        .find(|message| message["text"] == "всем от Боба")
        .unwrap();
    // Незнакомый Кэрол Боб подписан так, как его добавила Алиса.
    assert_eq!(bob_message["senderName"], "Собеседник");

    call_ok(&mut alice, json!({"command": "set_group_role", "group_id": group_id, "user_id": bob_id, "role": "admin"}));
    assert_eq!(bob.sync()["ok"], true);
    let bob_view = call_ok(&mut bob, json!({"command": "select_contact", "user_id": group_id}));
    assert_eq!(bob_view["snapshot"]["group"]["myRole"], "admin");
    assert_eq!(bob_view["snapshot"]["group"]["canRemoveMembers"], true);

    call_ok(&mut bob, json!({"command": "remove_group_member", "group_id": group_id, "user_id": carol_id}));
    for client in [&mut alice, &mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
    }
    let carol_view = call_ok(&mut carol, json!({"command": "select_contact", "user_id": group_id}));
    assert_eq!(carol_view["snapshot"]["group"]["left"], true, "{carol_view}");
    let alice_view = call_ok(&mut alice, json!({"command": "select_contact", "user_id": group_id}));
    assert_eq!(alice_view["snapshot"]["group"]["members"].as_array().unwrap().len(), 2);

    call_ok(&mut alice, json!({"command": "send_text", "user_id": group_id, "text": "уже без Кэрол"}));
    for client in [&mut alice, &mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
    }
    assert!(texts(&bob.conversation(&group_id)).contains(&"уже без Кэрол".to_owned()));
    assert!(!texts(&carol.conversation(&group_id)).contains(&"уже без Кэрол".to_owned()));
}

/// Файл уезжает в blob-хранилище и появляется у собеседника на диске.
#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn an_attachment_arrives_and_decrypts() {
    let Some(node) = node_url() else {
        panic!("Задайте TURAT_TEST_NODE");
    };
    let mut alice = Client::open("Алиса", &node);
    let mut bob = Client::open("Боб", &node);
    assert_eq!(alice.sync()["ok"], true);
    assert_eq!(bob.sync()["ok"], true);
    let bob_id = bob.user_id.clone();
    let alice_id = alice.user_id.clone();

    // Знакомство: вложения разрешены только в принятом диалоге.
    alice.add(&bob_id);
    assert_eq!(alice.send(&bob_id, "можно файл?")["ok"], true);
    assert_eq!(bob.sync()["ok"], true);
    assert_eq!(
        bob.core
            .call(&json!({"command": "accept_contact", "user_id": alice_id}))["ok"],
        true
    );
    assert_eq!(bob.send(&alice_id, "давай")["ok"], true);
    assert_eq!(alice.sync()["ok"], true);

    let source = std::env::temp_dir().join(format!("turat-file-{}.bin", uuid::Uuid::new_v4()));
    let content: Vec<u8> = (0..700_000u32).map(|value| (value % 251) as u8).collect();
    std::fs::write(&source, &content).expect("исходный файл");
    let attached = alice.core.call(&json!({
        "command": "attach_file",
        "user_id": bob_id,
        "path": source.to_string_lossy(),
        "mime_type": "application/octet-stream",
        "caption": "документ",
    }));
    assert_eq!(attached["ok"], true, "{attached}");

    assert_eq!(bob.sync()["ok"], true);
    let messages = bob.conversation(&alice_id);
    let last = messages.last().expect("сообщение с файлом");
    assert_eq!(last["text"], "документ");
    let local_path = PathBuf::from(
        last["attachment"]["localPath"]
            .as_str()
            .expect("путь к файлу"),
    );
    // Загрузка идёт фоном: ждём появления файла и сверяем содержимое.
    let mut ready = false;
    let mut status = String::new();
    for _ in 0..120 {
        if local_path.exists() {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
        status = bob.sync()["snapshot"]["statusMessage"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
    }
    assert!(ready, "файл не скачался: {local_path:?} ({status})");

    let destination = std::env::temp_dir().join(format!("turat-out-{}.bin", uuid::Uuid::new_v4()));
    let exported = bob.core.call(&json!({
        "command": "export_attachment",
        "event_id": last["eventId"],
        "destination_path": destination.to_string_lossy(),
    }));
    assert_eq!(exported["ok"], true, "{exported}");
    assert_eq!(std::fs::read(&destination).expect("выгруженный файл"), content);
}

/// Канал с подписчиками, незнакомыми владельцу: подписка по ссылке через публичный ящик,
/// история, пост, комментарий через администратора, просмотры и удаление подписчика.
#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_channel_reaches_subscribers_who_are_strangers() {
    let Some(node) = node_url() else {
        panic!("Задайте TURAT_TEST_NODE");
    };
    let mut alice = Client::open("Алиса", &node);
    let mut bob = Client::open("Боб", &node);
    let mut carol = Client::open("Кэрол", &node);
    for client in [&mut alice, &mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
    }
    let carol_id = carol.user_id.clone();

    let created = call_ok(&mut alice, json!({"command": "create_channel", "name": "Новости", "about": "Главное"}));
    let channel_id = created["value"]["channelId"].as_str().unwrap().to_owned();
    let link = created["snapshot"]["channel"]["inviteLink"].as_str().unwrap().to_owned();
    call_ok(&mut alice, json!({"command": "send_text", "user_id": channel_id, "text": "до подписки"}));

    for client in [&mut bob, &mut carol] {
        let subscribed = call_ok(client, json!({"command": "subscribe_channel", "link": link}));
        assert_eq!(subscribed["snapshot"]["channel"]["awaitingState"], true);
    }
    for _ in 0..2 {
        for client in [&mut alice, &mut bob, &mut carol] {
            assert_eq!(client.sync()["ok"], true);
        }
    }
    for client in [&mut bob, &mut carol] {
        let view = call_ok(client, json!({"command": "select_contact", "user_id": channel_id}));
        assert_eq!(view["snapshot"]["channel"]["awaitingState"], false, "{view}");
        assert_eq!(view["snapshot"]["channel"]["name"], "Новости");
        assert!(texts(&client.conversation(&channel_id)).contains(&"до подписки".to_owned()), "история не пришла");
    }

    call_ok(&mut alice, json!({"command": "send_text", "user_id": channel_id, "text": "свежий пост"}));
    for client in [&mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
    }
    let post = bob
        .conversation(&channel_id)
        .into_iter()
        .find(|message| message["text"] == "свежий пост")
        .expect("пост дошёл до Боба");
    let post_id = post["eventId"].as_str().unwrap().to_owned();

    call_ok(&mut bob, json!({"command": "mark_read", "user_id": channel_id}));
    call_ok(&mut bob, json!({"command": "send_comment", "post_event_id": post_id, "text": "спасибо!"}));
    assert_eq!(bob.sync()["ok"], true);
    assert_eq!(alice.sync()["ok"], true);
    assert_eq!(carol.sync()["ok"], true);
    let thread = call_ok(&mut carol, json!({"command": "open_comments", "post_event_id": post_id}));
    let comments = thread["snapshot"]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "{thread}");
    assert_eq!(comments[0]["text"], "спасибо!");
    assert_eq!(comments[0]["senderName"], "Боб");

    let owner_view = call_ok(&mut alice, json!({"command": "select_contact", "user_id": channel_id}));
    let own_post = owner_view["snapshot"]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| message["eventId"] == post_id)
        .cloned()
        .unwrap();
    assert_eq!(own_post["channelPost"]["views"], 1, "{own_post}");
    assert_eq!(own_post["channelPost"]["comments"], 1);
    assert_eq!(owner_view["snapshot"]["channel"]["subscriberCount"], 2);

    call_ok(&mut alice, json!({"command": "remove_channel_subscriber", "channel_id": channel_id, "user_id": carol_id}));
    call_ok(&mut alice, json!({"command": "send_text", "user_id": channel_id, "text": "уже без Кэрол"}));
    for client in [&mut bob, &mut carol] {
        assert_eq!(client.sync()["ok"], true);
    }
    assert!(texts(&bob.conversation(&channel_id)).contains(&"уже без Кэрол".to_owned()));
    assert!(!texts(&carol.conversation(&channel_id)).contains(&"уже без Кэрол".to_owned()));
    let carol_view = call_ok(&mut carol, json!({"command": "select_contact", "user_id": channel_id}));
    assert_eq!(carol_view["snapshot"]["channel"]["removed"], true, "{carol_view}");
}
