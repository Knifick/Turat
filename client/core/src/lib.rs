mod account;
mod blobs;
mod calls;
mod core;
mod identity;
mod mailbox;
mod media;
mod models;
mod network;
mod prekeys;
mod protocol;
mod ratchet;
mod routing;
mod store;

use std::{
    ffi::{CStr, CString, c_char},
    path::Path,
    sync::{Arc, Mutex},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use thiserror::Error;

use crate::core::AppCore;
use crate::network::{MailboxWatch, MailboxWatcher};

/// Тонкая обёртка для интеграционных тестов: они ходят в ядро тем же путём, что и
/// клиенты, — командой в JSON и снимком состояния в ответ. Включается флагом `testing`,
/// чтобы не попадать в библиотеку, которую грузят приложения.
#[cfg(feature = "testing")]
pub mod testing {
    use std::path::Path;

    use crate::core::AppCore;

    pub struct TestCore {
        core: AppCore,
    }

    impl TestCore {
        pub fn open(root: &Path) -> Self {
            Self {
                core: AppCore::open(root, [7u8; 32]).expect("ядро открывается"),
            }
        }

        pub fn call(&mut self, command: &serde_json::Value) -> serde_json::Value {
            serde_json::from_str(&self.core.invoke(&command.to_string()))
                .expect("ядро отвечает корректным JSON")
        }

        pub fn call_status(&self) -> serde_json::Value {
            serde_json::from_str(&crate::calls::status_json(&self.core.call_slot)).expect("JSON звонка")
        }

        pub fn call_push(&self, pcm: &[i16]) {
            crate::calls::push(&self.core.call_slot, pcm);
        }

        pub fn call_pull(&self, output: &mut [i16]) -> bool {
            crate::calls::pull(&self.core.call_slot, output)
        }

        pub fn call_action(&self, name: &str) -> bool {
            crate::calls::action(&self.core.call_slot, name)
        }
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    InvalidInput(String),
    #[error("Ошибка локального хранилища: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("Ошибка файла: {0}")]
    Io(#[from] std::io::Error),
    #[error("Ошибка JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Ошибка сети: {0}")]
    Network(#[from] reqwest::Error),
    /// Отказ Node с его собственным объяснением: код ответа пользователю ничего не говорит,
    /// а текст в теле говорит ровно то, что пошло не так.
    #[error("Node отказал: {0}")]
    Node(String),
    #[error("Ошибка Base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("Ошибка криптографии: {0}")]
    Crypto(String),
    /// Username на текущем Node принадлежит другой учётной записи.
    #[error("Username @{0} уже занят на этом Node — выберите другой")]
    UsernameTaken(String),
}

impl From<p256::pkcs8::Error> for CoreError {
    fn from(value: p256::pkcs8::Error) -> Self {
        Self::Crypto(value.to_string())
    }
}
impl From<p256::pkcs8::spki::Error> for CoreError {
    fn from(value: p256::pkcs8::spki::Error) -> Self {
        Self::Crypto(value.to_string())
    }
}
impl From<p256::ecdsa::Error> for CoreError {
    fn from(value: p256::ecdsa::Error) -> Self {
        Self::Crypto(value.to_string())
    }
}
pub struct CoreHandle {
    core: Mutex<AppCore>,
    /// Копия ключа хранилища: потоковый читатель вложений открывается без захвата ядра,
    /// иначе перемотка видео ждала бы очередную фоновую синхронизацию.
    vault_key: [u8; 32],
    /// Ожидание конверта на Node. Держать ради него замок ядра нельзя: пользователь не должен
    /// ждать конца окна, чтобы отправить сообщение.
    watch: Arc<Mutex<Option<MailboxWatch>>>,
    watcher: Option<MailboxWatcher>,
    /// Звонок: интерфейс и аудиопотоки обращаются к нему мимо замка ядра.
    calls: crate::calls::CallSlot,
}

impl CoreHandle {
    fn new(core: AppCore, vault_key: [u8; 32]) -> Self {
        Self {
            watch: core.watch_handle(),
            calls: core.call_slot.clone(),
            core: Mutex::new(core),
            vault_key,
            watcher: MailboxWatcher::new().ok(),
        }
    }

    /// Одно окно ожидания. 1 — в ящике появился конверт, пора синхронизироваться;
    /// 0 — окно истекло впустую; -1 — ждать пока негде или связь оборвалась.
    fn wait_for_envelopes(&self, seconds: i32) -> i32 {
        let Some(watcher) = self.watcher.as_ref() else {
            return -1;
        };
        let target = match self.watch.lock() {
            Ok(slot) => slot.clone(),
            Err(_) => None,
        };
        let Some(target) = target else {
            return -1;
        };
        let window = seconds.clamp(1, 600) as u32;
        let started = std::time::Instant::now();
        match watcher.wait(&target, window) {
            Ok(true) => 1,
            // Node, который не умеет ждать, отвечает пустым списком сразу. Признать это
            // окончанием окна нельзя: цикл клиента начал бы долбить Node без пауз.
            Ok(false) if started.elapsed().as_secs() * 2 < window as u64 => -1,
            Ok(false) => 0,
            Err(_) => -1,
        }
    }
}

/// Открытое вложение: читается по произвольному смещению, расшифровывая только нужные чанки.
pub struct MediaHandle(Mutex<crate::media::MediaReader>);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_core_create(
    app_dir: *const c_char,
    vault_key_base64: *const c_char,
) -> *mut CoreHandle {
    let result = (|| {
        let path = unsafe { CStr::from_ptr(app_dir) }.to_str().ok()?;
        let encoded = unsafe { CStr::from_ptr(vault_key_base64) }.to_str().ok()?;
        let bytes = STANDARD.decode(encoded).ok()?;
        let key: [u8; 32] = bytes.try_into().ok()?;
        let core = AppCore::open(Path::new(path), key).ok()?;
        Some(CoreHandle::new(core, key))
    })();
    result
        .map(|handle| Box::into_raw(Box::new(handle)))
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_core_invoke(
    handle: *mut CoreHandle,
    request_json: *const c_char,
) -> *mut c_char {
    if handle.is_null() || request_json.is_null() {
        return json_string("Некорректный вызов Rust core");
    }
    let request = match unsafe { CStr::from_ptr(request_json) }.to_str() {
        Ok(value) => value,
        Err(_) => return json_string("Команда не является UTF-8"),
    };
    let core = unsafe { &*handle };
    let response = match core.core.lock() {
        Ok(mut value) => value.invoke(request),
        Err(_) => "{\"ok\":false,\"error\":\"Rust core lock poisoned\"}".to_owned(),
    };
    CString::new(response)
        .map(CString::into_raw)
        .unwrap_or_else(|_| json_string("Ответ содержит NUL"))
}

/// Блокирующее ожидание входящего конверта: Node держит запрос открытым и отвечает сразу,
/// как только сообщение приходит. Вызывается из фонового потока — замок ядра при этом
/// свободен, поэтому отправка и действия пользователя не ждут.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_core_wait_for_envelopes(
    handle: *mut CoreHandle,
    seconds: i32,
) -> i32 {
    if handle.is_null() {
        return -1;
    }
    unsafe { &*handle }.wait_for_envelopes(seconds)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_core_destroy(handle: *mut CoreHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_string_free(value: *mut c_char) {
    if !value.is_null() {
        drop(unsafe { CString::from_raw(value) });
    }
}

/// Открывает вложение для потокового чтения. Плеер и просмотрщик картинок читают файл
/// кусками, а не расшифровывают его целиком: 300-мегабайтное видео стартует сразу.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_media_open(
    handle: *mut CoreHandle,
    path: *const c_char,
) -> *mut MediaHandle {
    if handle.is_null() || path.is_null() {
        return std::ptr::null_mut();
    }
    let core = unsafe { &*handle };
    let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
        return std::ptr::null_mut();
    };
    match crate::media::MediaReader::open(&core.vault_key, Path::new(path)) {
        Ok(reader) => Box::into_raw(Box::new(MediaHandle(Mutex::new(reader)))),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Длина расшифрованного вложения в байтах, -1 при ошибке.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_media_length(handle: *mut MediaHandle) -> i64 {
    if handle.is_null() {
        return -1;
    }
    let media = unsafe { &*handle };
    media
        .0
        .lock()
        .map(|reader| reader.length() as i64)
        .unwrap_or(-1)
}

/// Читает до `length` байт с позиции `offset`; возвращает прочитанное количество,
/// 0 в конце файла и -1 при ошибке.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_media_read(
    handle: *mut MediaHandle,
    offset: u64,
    buffer: *mut u8,
    length: usize,
) -> i64 {
    if handle.is_null() || buffer.is_null() {
        return -1;
    }
    let media = unsafe { &*handle };
    let output = unsafe { std::slice::from_raw_parts_mut(buffer, length) };
    match media.0.lock() {
        Ok(mut reader) => reader.read_at(offset, output).map(|v| v as i64).unwrap_or(-1),
        Err(_) => -1,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_media_close(handle: *mut MediaHandle) {
    if !handle.is_null() {
        drop(unsafe { Box::from_raw(handle) });
    }
}

/// Кадр микрофона для текущего звонка: 960 отсчётов 16 бит, 48 кГц, моно. Замок ядра не
/// нужен — аудиопоток не ждёт синхронизацию переписки.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_call_push(handle: *mut CoreHandle, pcm: *const i16, length: usize) {
    if handle.is_null() || pcm.is_null() {
        return;
    }
    crate::calls::push(&unsafe { &*handle }.calls, unsafe { std::slice::from_raw_parts(pcm, length) });
}

/// Кадр для динамика. 1 — звонок идёт, 0 — звонка нет (в буфере тишина).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_call_pull(handle: *mut CoreHandle, output: *mut i16, length: usize) -> i32 {
    if handle.is_null() || output.is_null() {
        return 0;
    }
    i32::from(crate::calls::pull(&unsafe { &*handle }.calls, unsafe { std::slice::from_raw_parts_mut(output, length) }))
}

/// Состояние звонка в JSON. Освобождается через `turattext_string_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_call_status(handle: *mut CoreHandle) -> *mut c_char {
    if handle.is_null() {
        return json_string("Rust core не запущен");
    }
    CString::new(crate::calls::status_json(&unsafe { &*handle }.calls))
        .map(CString::into_raw)
        .unwrap_or_else(|_| json_string("Ответ содержит NUL"))
}

/// Действие без ядра: `mute`, `unmute`, `hangup`, `dismiss`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn turattext_call_action(handle: *mut CoreHandle, name: *const c_char) -> i32 {
    if handle.is_null() || name.is_null() {
        return 0;
    }
    match unsafe { CStr::from_ptr(name) }.to_str() {
        Ok(value) => i32::from(crate::calls::action(&unsafe { &*handle }.calls, value)),
        Err(_) => 0,
    }
}

fn json_string(error: &str) -> *mut c_char {
    let json = serde_json::json!({"ok":false,"error":error}).to_string();
    CString::new(json).expect("static JSON").into_raw()
}

#[cfg(target_os = "android")]
mod android {
    use jni::{
        JNIEnv,
        objects::{JByteArray, JClass, JShortArray, JString},
        sys::{jboolean, jint, jlong, jstring},
    };

    use super::*;

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeCreate(
        mut env: JNIEnv,
        _class: JClass,
        app_dir: JString,
        key: JString,
    ) -> jlong {
        let path: String = match env.get_string(&app_dir) {
            Ok(v) => v.into(),
            Err(_) => return 0,
        };
        let encoded: String = match env.get_string(&key) {
            Ok(v) => v.into(),
            Err(_) => return 0,
        };
        let bytes = match STANDARD.decode(encoded) {
            Ok(v) => v,
            Err(_) => return 0,
        };
        let key: [u8; 32] = match bytes.try_into() {
            Ok(v) => v,
            Err(_) => return 0,
        };
        AppCore::open(Path::new(&path), key)
            .map(|core| Box::into_raw(Box::new(CoreHandle::new(core, key))) as jlong)
            .unwrap_or(0)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeInvoke(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        request: JString,
    ) -> jstring {
        let request: String = match env.get_string(&request) {
            Ok(v) => v.into(),
            Err(_) => "{}".to_owned(),
        };
        let response = if handle == 0 {
            "{\"ok\":false,\"error\":\"Rust core не запущен\"}".to_owned()
        } else {
            let core = unsafe { &*(handle as *mut CoreHandle) };
            core.core
                .lock()
                .map(|mut value| value.invoke(&request))
                .unwrap_or_else(|_| {
                    "{\"ok\":false,\"error\":\"Rust core lock poisoned\"}".to_owned()
                })
        };
        env.new_string(response)
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeWaitForEnvelopes(
        _env: JNIEnv,
        _class: JClass,
        handle: jlong,
        seconds: jint,
    ) -> jint {
        if handle == 0 {
            return -1;
        }
        unsafe { &*(handle as *mut CoreHandle) }.wait_for_envelopes(seconds)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeMediaOpen(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        path: JString,
    ) -> jlong {
        if handle == 0 {
            return 0;
        }
        let path: String = match env.get_string(&path) {
            Ok(v) => v.into(),
            Err(_) => return 0,
        };
        let core = unsafe { &*(handle as *mut CoreHandle) };
        crate::media::MediaReader::open(&core.vault_key, Path::new(&path))
            .map(|reader| Box::into_raw(Box::new(MediaHandle(Mutex::new(reader)))) as jlong)
            .unwrap_or(0)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeMediaLength(
        _env: JNIEnv,
        _class: JClass,
        media: jlong,
    ) -> jlong {
        if media == 0 {
            return -1;
        }
        let handle = unsafe { &*(media as *mut MediaHandle) };
        handle
            .0
            .lock()
            .map(|reader| reader.length() as jlong)
            .unwrap_or(-1)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeMediaRead(
        mut env: JNIEnv,
        _class: JClass,
        media: jlong,
        offset: jlong,
        buffer: JByteArray,
        length: jint,
    ) -> jint {
        if media == 0 || offset < 0 || length <= 0 {
            return -1;
        }
        let handle = unsafe { &*(media as *mut MediaHandle) };
        let mut scratch = vec![0u8; length as usize];
        let read = match handle.0.lock() {
            Ok(mut reader) => match reader.read_at(offset as u64, &mut scratch) {
                Ok(value) => value,
                Err(_) => return -1,
            },
            Err(_) => return -1,
        };
        if read == 0 {
            return 0;
        }
        let signed: &[i8] =
            unsafe { std::slice::from_raw_parts(scratch.as_ptr() as *const i8, read) };
        match env.set_byte_array_region(&buffer, 0, signed) {
            Ok(()) => read as jint,
            Err(_) => -1,
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeMediaClose(
        _env: JNIEnv,
        _class: JClass,
        media: jlong,
    ) {
        if media != 0 {
            drop(unsafe { Box::from_raw(media as *mut MediaHandle) });
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeCallPush(
        env: JNIEnv,
        _class: JClass,
        handle: jlong,
        pcm: JShortArray,
        length: jint,
    ) {
        if handle == 0 {
            return;
        }
        let calls = &unsafe { &*(handle as *mut CoreHandle) }.calls;
        let length = length.clamp(0, 4096) as usize;
        let mut frame = vec![0i16; length];
        if env.get_short_array_region(&pcm, 0, &mut frame).is_ok() {
            crate::calls::push(calls, &frame);
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeCallPull(
        env: JNIEnv,
        _class: JClass,
        handle: jlong,
        output: JShortArray,
        length: jint,
    ) -> jboolean {
        if handle == 0 {
            return 0;
        }
        let calls = &unsafe { &*(handle as *mut CoreHandle) }.calls;
        let length = length.clamp(0, 4096) as usize;
        let mut frame = vec![0i16; length];
        let active = crate::calls::pull(calls, &mut frame);
        let _ = env.set_short_array_region(&output, 0, &frame);
        u8::from(active)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeCallStatus(
        env: JNIEnv,
        _class: JClass,
        handle: jlong,
    ) -> jstring {
        let status = if handle == 0 {
            "{\"active\":false}".to_owned()
        } else {
            crate::calls::status_json(&unsafe { &*(handle as *mut CoreHandle) }.calls)
        };
        env.new_string(status)
            .map(|value| value.into_raw())
            .unwrap_or(std::ptr::null_mut())
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeCallAction(
        mut env: JNIEnv,
        _class: JClass,
        handle: jlong,
        name: JString,
    ) -> jboolean {
        if handle == 0 {
            return 0;
        }
        let name: String = match env.get_string(&name) {
            Ok(value) => value.into(),
            Err(_) => return 0,
        };
        u8::from(crate::calls::action(&unsafe { &*(handle as *mut CoreHandle) }.calls, &name))
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_app_turattext_mobile_core_NativeCore_nativeDestroy(
        _env: JNIEnv,
        _class: JClass,
        handle: jlong,
    ) {
        if handle != 0 {
            drop(unsafe { Box::from_raw(handle as *mut CoreHandle) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_round_trip() {
        let root = std::env::temp_dir().join(format!("turattext-core-{}", uuid::Uuid::new_v4()));
        let mut core = AppCore::open(&root, [7u8; 32]).unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&core.invoke(r#"{"command":"snapshot"}"#)).unwrap();
        assert_eq!(response["ok"], true);
        assert!(
            response["snapshot"]["identity"]["userId"]
                .as_str()
                .unwrap()
                .starts_with("tt1-")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn profile_and_identity_survive_restart() {
        let root = std::env::temp_dir().join(format!("turattext-core-{}", uuid::Uuid::new_v4()));
        let user_id = {
            let mut core = AppCore::open(&root, [9u8; 32]).unwrap();
            let response: serde_json::Value = serde_json::from_str(&core.invoke(
                r#"{"command":"save_profile","username":"native_user","display_name":"Native User","about":"Rust","avatar_base64":null}"#,
            ))
            .unwrap();
            assert_eq!(response["ok"], true);
            response["snapshot"]["identity"]["userId"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        let mut reopened = AppCore::open(&root, [9u8; 32]).unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&reopened.invoke(r#"{"command":"snapshot"}"#)).unwrap();
        assert_eq!(response["snapshot"]["identity"]["userId"], user_id);
        assert_eq!(
            response["snapshot"]["profile"]["displayName"],
            "Native User"
        );
        assert!(AppCore::open(&root, [8u8; 32]).is_err());
        std::fs::remove_dir_all(root).ok();
    }

    /// Пустой выбор чата пишет в meta JSON `null` — раньше это ломало следующий запуск.
    #[test]
    fn empty_selection_survives_restart() {
        let root = std::env::temp_dir().join(format!("turattext-core-{}", uuid::Uuid::new_v4()));
        {
            let mut core = AppCore::open(&root, [11u8; 32]).unwrap();
            let response: serde_json::Value = serde_json::from_str(
                &core.invoke(r#"{"command":"select_contact","user_id":null}"#),
            )
            .unwrap();
            assert_eq!(response["ok"], true);
        }
        let mut reopened = AppCore::open(&root, [11u8; 32]).unwrap();
        let response: serde_json::Value =
            serde_json::from_str(&reopened.invoke(r#"{"command":"snapshot"}"#)).unwrap();
        assert_eq!(response["ok"], true);
        assert!(response["snapshot"]["selectedContactId"].is_null());
        assert!(response["snapshot"]["chats"].is_array());
        std::fs::remove_dir_all(root).ok();
    }
}
