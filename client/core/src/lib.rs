mod core;
mod identity;
mod models;
mod network;
mod store;

use std::{
    ffi::{CStr, CString, c_char},
    path::Path,
    sync::Mutex,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use thiserror::Error;

use crate::core::AppCore;

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
    #[error("Ошибка Base64: {0}")]
    Base64(#[from] base64::DecodeError),
    #[error("Ошибка криптографии: {0}")]
    Crypto(String),
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
pub struct CoreHandle(Mutex<AppCore>);

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
        AppCore::open(Path::new(path), key).ok()
    })();
    result
        .map(|core| Box::into_raw(Box::new(CoreHandle(Mutex::new(core)))))
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
    let response = match core.0.lock() {
        Ok(mut value) => value.invoke(request),
        Err(_) => "{\"ok\":false,\"error\":\"Rust core lock poisoned\"}".to_owned(),
    };
    CString::new(response)
        .map(CString::into_raw)
        .unwrap_or_else(|_| json_string("Ответ содержит NUL"))
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

fn json_string(error: &str) -> *mut c_char {
    let json = serde_json::json!({"ok":false,"error":error}).to_string();
    CString::new(json).expect("static JSON").into_raw()
}

#[cfg(target_os = "android")]
mod android {
    use jni::{
        JNIEnv,
        objects::{JClass, JString},
        sys::{jlong, jstring},
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
            .map(|core| Box::into_raw(Box::new(CoreHandle(Mutex::new(core)))) as jlong)
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
            core.0
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
