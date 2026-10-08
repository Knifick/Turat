//! Сквозная проверка аккаунтов и синхронизации устройств через настоящий Node.
//!
//! Без флага `testing` файл превращается в пустой: обычная сборка не должна зависеть
//! от тестовой обёртки над ядром.
#![cfg(feature = "testing")]

//!
//! Нужен живой узел с PostgreSQL:
//! `TURAT_TEST_NODE=http://localhost:18080 cargo test --features testing --test accounts -- --ignored --test-threads=1`

use serde_json::{Value, json};
use turattext_core::testing::TestCore;

fn node_url() -> String {
    std::env::var("TURAT_TEST_NODE").expect("TURAT_TEST_NODE")
}

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

struct Device {
    core: TestCore,
}

impl Device {
    fn open(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("turat-acc-{name}-{}", uuid::Uuid::new_v4()));
        let mut core = TestCore::open(&root);
        let connected = core.call(&json!({"command": "connect", "bootstrap_url": node_url()}));
        assert_eq!(connected["ok"], true, "подключение к Node: {connected}");
        Self { core }
    }

    fn call(&mut self, command: Value) -> Value {
        self.core.call(&command)
    }

    fn ok(&mut self, command: Value) -> Value {
        let response = self.call(command.clone());
        assert_eq!(response["ok"], true, "{command} → {response}");
        response
    }

    fn sync(&mut self) -> Value {
        self.ok(json!({"command": "sync"}))
    }

    fn snapshot(&mut self) -> Value {
        self.ok(json!({"command": "snapshot"}))["snapshot"].clone()
    }

    fn user_id(&mut self) -> String {
        self.snapshot()["identity"]["userId"].as_str().unwrap().to_owned()
    }

    fn register(&mut self, username: &str, password: &str) -> String {
        let response = self.ok(json!({
            "command": "account_register",
            "username": username,
            "display_name": "Алиса",
            "password": password,
            "device_name": "Телефон",
        }));
        response["value"]["recoveryKey"].as_str().unwrap().to_owned()
    }

    fn conversation(&mut self, peer: &str) -> Vec<Value> {
        self.ok(json!({"command": "select_contact", "user_id": peer}))["snapshot"]["messages"]
            .as_array()
            .cloned()
            .unwrap_or_default()
    }

    fn texts(&mut self, peer: &str) -> Vec<String> {
        self.conversation(peer)
            .iter()
            .filter(|message| message["service"] != true)
            .map(|message| message["text"].as_str().unwrap_or_default().to_owned())
            .collect()
    }
}

/// Собеседник «старого образца»: профиль без аккаунта, как у установок до этой версии.
fn legacy_peer(name: &str) -> (Device, String) {
    let mut peer = Device::open(name);
    peer.ok(json!({
        "command": "save_profile",
        "username": "",
        "display_name": name,
        "about": "",
        "avatar_base64": Value::Null,
    }));
    peer.sync();
    let id = peer.user_id();
    (peer, id)
}

fn pump(devices: &mut [&mut Device], rounds: usize) {
    for _ in 0..rounds {
        for device in devices.iter_mut() {
            device.sync();
        }
    }
}

#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_second_device_logs_in_and_stays_in_sync() {
    let username = unique("alice");
    let password = "очень-надёжный-пароль";
    let mut phone = Device::open("phone");
    assert_eq!(phone.snapshot()["account"]["state"], "none");
    assert_eq!(phone.snapshot()["onboardingRequired"], true);
    let recovery = phone.register(&username, password);
    assert_eq!(recovery.len(), 39, "{recovery}");
    let view = phone.snapshot()["account"].clone();
    assert_eq!(view["state"], "active", "{view}");
    assert_eq!(view["recoveryKey"], recovery);
    phone.ok(json!({"command": "account_confirm_recovery_key"}));
    assert!(phone.snapshot()["account"]["recoveryKey"].is_null());
    let alice = phone.user_id();

    // Переписка до появления второго устройства.
    let (mut bob, bob_id) = legacy_peer("Боб");
    phone.ok(json!({"command": "add_contact", "query": bob_id, "display_name": "Боб"}));
    phone.ok(json!({"command": "send_text", "user_id": bob_id, "text": "Привет, Боб"}));
    pump(&mut [&mut phone, &mut bob], 2);
    bob.ok(json!({"command": "accept_contact", "user_id": alice}));
    bob.ok(json!({"command": "send_text", "user_id": alice, "text": "Привет, Алиса"}));
    pump(&mut [&mut bob, &mut phone], 2);
    assert_eq!(phone.texts(&bob_id), ["Привет, Боб", "Привет, Алиса"]);

    // Второе устройство: вход по username и паролю — без файлов.
    let mut laptop = Device::open("laptop");
    laptop.ok(json!({
        "command": "account_login",
        "username": username,
        "password": password,
        "device_name": "Ноутбук",
    }));
    let snapshot = laptop.snapshot();
    assert_eq!(snapshot["account"]["state"], "active");
    assert_eq!(snapshot["identity"]["userId"], alice, "та же личность");
    assert_ne!(
        snapshot["identity"]["deviceId"],
        phone.snapshot()["identity"]["deviceId"],
        "но своё устройство"
    );
    assert_eq!(snapshot["profile"]["displayName"], "Алиса");

    // Телефон узнаёт о ноутбуке и выкладывает свежий снимок, Боб — узнаёт о новом устройстве.
    pump(&mut [&mut phone, &mut laptop, &mut bob], 3);
    assert_eq!(laptop.texts(&bob_id), ["Привет, Боб", "Привет, Алиса"], "история с телефона");
    let devices = phone.snapshot()["account"]["devices"].as_array().unwrap().len();
    assert_eq!(devices, 2, "{}", phone.snapshot()["account"]);

    // Отправленное с телефона появляется на ноутбуке.
    phone.ok(json!({"command": "send_text", "user_id": bob_id, "text": "С телефона"}));
    pump(&mut [&mut phone, &mut laptop, &mut bob], 3);
    assert!(laptop.texts(&bob_id).contains(&"С телефона".to_owned()), "{:?}", laptop.texts(&bob_id));

    // Ответ Боба приходит на оба устройства.
    bob.ok(json!({"command": "send_text", "user_id": alice, "text": "Обоим"}));
    pump(&mut [&mut bob, &mut phone, &mut laptop], 3);
    assert!(phone.texts(&bob_id).contains(&"Обоим".to_owned()));
    assert!(laptop.texts(&bob_id).contains(&"Обоим".to_owned()));

    // Отправленное с ноутбука видит телефон, и Боб получает его.
    laptop.ok(json!({"command": "send_text", "user_id": bob_id, "text": "С ноутбука"}));
    pump(&mut [&mut laptop, &mut phone, &mut bob], 3);
    assert!(phone.texts(&bob_id).contains(&"С ноутбука".to_owned()));
    assert!(bob.texts(&alice).contains(&"С ноутбука".to_owned()));

    // Закреплённый чат — пример изменения, которое делает сам пользователь.
    laptop.ok(json!({"command": "set_chat_pinned", "user_id": bob_id, "pinned": true}));
    pump(&mut [&mut laptop, &mut phone], 2);
    let pinned = phone.snapshot()["chats"]
        .as_array()
        .unwrap()
        .iter()
        .find(|chat| chat["userId"] == bob_id)
        .map(|chat| chat["pinned"] == true);
    assert_eq!(pinned, Some(true));

    // Выход на ноутбуке: там всё стирается, телефон убирает его из списка устройств.
    laptop.ok(json!({"command": "account_logout"}));
    let after = laptop.snapshot();
    assert_eq!(after["account"]["state"], "none");
    assert!(after["chats"].as_array().unwrap().is_empty(), "данные стёрты");
    pump(&mut [&mut phone], 1);
    assert_eq!(phone.snapshot()["account"]["devices"].as_array().unwrap().len(), 1);
    // Боб по-прежнему пишет телефону.
    bob.sync();
    bob.ok(json!({"command": "send_text", "user_id": alice, "text": "После выхода ноутбука"}));
    pump(&mut [&mut bob, &mut phone], 2);
    assert!(phone.texts(&bob_id).contains(&"После выхода ноутбука".to_owned()));
}

#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_revoked_device_signs_itself_out() {
    let username = unique("carol");
    let password = "пароль-для-проверки";
    let mut first = Device::open("first");
    first.register(&username, password);
    first.sync();
    let mut second = Device::open("second");
    second.ok(json!({"command": "account_login", "username": username, "password": password}));
    pump(&mut [&mut first, &mut second], 2);
    let second_device = second.snapshot()["identity"]["deviceId"].as_str().unwrap().to_owned();
    first.ok(json!({"command": "revoke_device", "device_id": second_device}));
    // Второе устройство узнаёт об этом при сверке списка устройств.
    let response = second.call(json!({"command": "sync"}));
    assert_eq!(response["ok"], true, "{response}");
    let view = second.snapshot()["account"].clone();
    assert_eq!(view["state"], "none", "{view}");
    assert!(view["notice"].as_str().unwrap_or_default().contains("завершён"), "{view}");
}

#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn wrong_password_fails_and_the_recovery_key_restores_access() {
    let username = unique("dave");
    let mut owner = Device::open("owner");
    let recovery = owner.register(&username, "старый-пароль-1");
    owner.sync();

    let mut stranger = Device::open("stranger");
    let failed = stranger.call(json!({
        "command": "account_login",
        "username": username,
        "password": "не-тот-пароль",
    }));
    assert_eq!(failed["ok"], false);
    assert!(failed["error"].as_str().unwrap().contains("Неверный логин или пароль"), "{failed}");
    assert_eq!(stranger.snapshot()["account"]["state"], "none");

    // Опечатку в ключе ловит контрольная сумма ещё до Node.
    let mut typo: Vec<char> = recovery.chars().collect();
    typo[0] = if typo[0] == 'A' { 'B' } else { 'A' };
    let typed: String = typo.into_iter().collect();
    let rejected = stranger.call(json!({
        "command": "account_recover",
        "recovery_key": typed,
        "new_password": "новый-пароль-2",
    }));
    assert_eq!(rejected["ok"], false, "{rejected}");

    let mut restored = Device::open("restored");
    let response = restored.ok(json!({
        "command": "account_recover",
        "recovery_key": recovery.to_lowercase().replace('-', " "),
        "new_password": "новый-пароль-2",
        "device_name": "Восстановленный",
    }));
    let new_key = response["value"]["recoveryKey"].as_str().unwrap().to_owned();
    assert_ne!(new_key, recovery, "использованный ключ заменён новым");
    assert_eq!(restored.snapshot()["account"]["state"], "active");
    assert_eq!(restored.user_id(), owner.user_id());

    // Старый пароль больше не подходит, новый — подходит.
    let mut again = Device::open("again");
    let old = again.call(json!({"command": "account_login", "username": username, "password": "старый-пароль-1"}));
    assert_eq!(old["ok"], false);
    again.ok(json!({"command": "account_login", "username": username, "password": "новый-пароль-2"}));
    // И старый ключ восстановления тоже.
    let mut late = Device::open("late");
    let stale = late.call(json!({"command": "account_recover", "recovery_key": recovery, "new_password": "ещё-один-пароль"}));
    assert_eq!(stale["ok"], false, "{stale}");

    // Смена пароля требует текущий пароль.
    let wrong = again.call(json!({"command": "account_change_password", "old_password": "мимо-мимо", "new_password": "третий-пароль-3"}));
    assert_eq!(wrong["ok"], false);
    again.ok(json!({"command": "account_change_password", "old_password": "новый-пароль-2", "new_password": "третий-пароль-3"}));
    let mut last = Device::open("last");
    last.ok(json!({"command": "account_login", "username": username, "password": "третий-пароль-3"}));
}

#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_username_belongs_to_one_account_per_node() {
    let username = unique("erin");
    let mut first = Device::open("erin1");
    first.register(&username, "пароль-первого");
    let checked = first.ok(json!({"command": "check_username", "username": username}));
    assert_eq!(checked["value"]["available"], true, "своё имя свободно для себя");

    let mut second = Device::open("erin2");
    let checked = second.ok(json!({"command": "check_username", "username": username}));
    assert_eq!(checked["value"]["available"], false);
    let taken = second.call(json!({
        "command": "account_register",
        "username": username,
        "display_name": "Другой",
        "password": "пароль-второго",
    }));
    assert_eq!(taken["ok"], false);
    assert!(taken["error"].as_str().unwrap().contains("занят"), "{taken}");
    second.register(&unique("erin_other"), "пароль-второго");

    // Смена username: старое имя освобождается, вход — уже по новому.
    let renamed = unique("erin_new");
    first.ok(json!({
        "command": "save_profile",
        "username": renamed,
        "display_name": "Эрин",
        "about": "",
        "avatar_base64": Value::Null,
    }));
    first.ok(json!({"command": "publish_profile"}));
    let mut third = Device::open("erin3");
    third.ok(json!({"command": "account_login", "username": renamed, "password": "пароль-первого"}));
}

#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn an_existing_profile_gets_an_account_without_losing_chats() {
    let (mut old, old_id) = legacy_peer("Старый");
    let (mut peer, peer_id) = legacy_peer("Собеседник");
    old.ok(json!({"command": "add_contact", "query": peer_id, "display_name": "Собеседник"}));
    old.ok(json!({"command": "send_text", "user_id": peer_id, "text": "Ещё до аккаунтов"}));
    pump(&mut [&mut old, &mut peer], 2);
    assert_eq!(old.snapshot()["account"]["state"], "legacy");

    let username = unique("legacy");
    let response = old.ok(json!({
        "command": "account_register",
        "username": username,
        "password": "пароль-старожила",
    }));
    assert!(response["value"]["recoveryKey"].is_string());
    assert_eq!(old.user_id(), old_id, "личность не меняется");
    assert_eq!(old.texts(&peer_id), ["Ещё до аккаунтов"]);
    assert_eq!(old.snapshot()["profile"]["displayName"], "Старый");

    // На новом устройстве — та же переписка.
    let mut fresh = Device::open("fresh");
    fresh.ok(json!({"command": "account_login", "username": username, "password": "пароль-старожила"}));
    assert_eq!(fresh.texts(&peer_id), ["Ещё до аккаунтов"]);

    // А войти на устройстве со своей перепиской можно только с явного согласия.
    let (mut busy, _) = legacy_peer("Занятой");
    let refused = busy.call(json!({"command": "account_login", "username": username, "password": "пароль-старожила"}));
    assert_eq!(refused["ok"], false, "{refused}");
    busy.ok(json!({
        "command": "account_login",
        "username": username,
        "password": "пароль-старожила",
        "discard_local": true,
    }));
    assert_eq!(busy.user_id(), old_id);
}

/// Группа на двух устройствах: сообщения участников доходят до обоих, а написанное с одного
/// устройства видно на другом.
#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_group_works_on_both_devices() {
    let username = unique("grace");
    let password = "пароль-для-групп";
    let mut phone = Device::open("grace-phone");
    phone.register(&username, password);
    phone.ok(json!({"command": "account_confirm_recovery_key"}));
    let grace = phone.user_id();
    let (mut bob, bob_id) = legacy_peer("Боб");
    phone.ok(json!({"command": "add_contact", "query": bob_id, "display_name": "Боб"}));
    phone.ok(json!({"command": "send_text", "user_id": bob_id, "text": "привет"}));
    pump(&mut [&mut phone, &mut bob], 2);
    bob.ok(json!({"command": "accept_contact", "user_id": grace}));
    pump(&mut [&mut bob, &mut phone], 2);

    let created = phone.ok(json!({"command": "create_group", "name": "Двое и устройства", "member_ids": [bob_id]}));
    let group_id = created["value"]["groupId"].as_str().unwrap().to_owned();
    pump(&mut [&mut phone, &mut bob], 2);
    bob.ok(json!({"command": "accept_contact", "user_id": group_id}));
    pump(&mut [&mut bob, &mut phone], 2);

    let mut laptop = Device::open("grace-laptop");
    laptop.ok(json!({"command": "account_login", "username": username, "password": password}));
    pump(&mut [&mut phone, &mut laptop, &mut bob], 3);
    let view = laptop.ok(json!({"command": "select_contact", "user_id": group_id}))["snapshot"]["group"].clone();
    assert_eq!(view["name"], "Двое и устройства", "группа пришла на ноутбук: {view}");

    bob.ok(json!({"command": "send_text", "user_id": group_id, "text": "в группу от Боба"}));
    pump(&mut [&mut bob, &mut phone, &mut laptop], 3);
    assert!(phone.texts(&group_id).contains(&"в группу от Боба".to_owned()));
    assert!(laptop.texts(&group_id).contains(&"в группу от Боба".to_owned()));

    laptop.ok(json!({"command": "send_text", "user_id": group_id, "text": "в группу с ноутбука"}));
    pump(&mut [&mut laptop, &mut phone, &mut bob], 3);
    assert!(bob.texts(&group_id).contains(&"в группу с ноутбука".to_owned()), "{:?}", bob.texts(&group_id));
    assert!(phone.texts(&group_id).contains(&"в группу с ноутбука".to_owned()), "{:?}", phone.texts(&group_id));
}

/// Отозванное устройство, ещё не узнавшее об этом, не может ни писать от имени аккаунта
/// собеседникам, ни менять данные на других устройствах.
#[test]
#[ignore = "нужен запущенный Node: TURAT_TEST_NODE"]
fn a_revoked_device_can_no_longer_speak_for_the_account() {
    let username = unique("heidi");
    let password = "пароль-для-отзыва";
    let mut phone = Device::open("heidi-phone");
    phone.register(&username, password);
    phone.ok(json!({"command": "account_confirm_recovery_key"}));
    let heidi = phone.user_id();
    let (mut bob, bob_id) = legacy_peer("Боб");
    phone.ok(json!({"command": "add_contact", "query": bob_id, "display_name": "Боб"}));
    phone.ok(json!({"command": "send_text", "user_id": bob_id, "text": "привет"}));
    pump(&mut [&mut phone, &mut bob], 2);
    bob.ok(json!({"command": "accept_contact", "user_id": heidi}));
    pump(&mut [&mut bob, &mut phone], 2);

    let mut stolen = Device::open("heidi-stolen");
    stolen.ok(json!({"command": "account_login", "username": username, "password": password}));
    pump(&mut [&mut phone, &mut stolen, &mut bob], 3);

    // Сеанс завершают, пока украденное устройство не в сети: о нём оно не узнало.
    let stolen_device = stolen.snapshot()["identity"]["deviceId"].as_str().unwrap().to_owned();
    phone.ok(json!({"command": "revoke_device", "device_id": stolen_device}));
    // Боб узнаёт о новом списке устройств из любого события телефона.
    phone.ok(json!({"command": "send_text", "user_id": bob_id, "text": "сеанс завершён"}));
    pump(&mut [&mut phone, &mut bob], 2);

    // Украденное устройство пишет, не синхронизируясь: со старым списком, где оно ещё есть.
    stolen.ok(json!({"command": "send_text", "user_id": bob_id, "text": "от отозванного"}));
    bob.sync();
    assert!(!bob.texts(&heidi).contains(&"от отозванного".to_owned()), "{:?}", bob.texts(&heidi));
    phone.sync();
    assert!(!phone.texts(&bob_id).contains(&"от отозванного".to_owned()), "{:?}", phone.texts(&bob_id));
}
