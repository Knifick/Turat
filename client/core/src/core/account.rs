//! Учётная запись на устройстве: регистрация, вход по паролю, восстановление по ключу, выход.
//!
//! Криптография — в [`crate::account`], синхронизация данных — в [`super::sync`]. Здесь — то, как
//! устройство становится частью аккаунта и перестаёт ею быть.

use serde_json::json;

use super::{AppCore, sync::encode_key};
use crate::{
    CoreError,
    account::{
        self, DeviceName, LocalAccount, PURPOSE_PASSWORD_WRAP, PURPOSE_RECOVERY_WRAP,
        ServerCredentials, VaultContent,
    },
    calls,
    identity::StoredIdentity,
    models::{AccountDeviceView, AccountView},
    network::{AccountSecretsWire, NodeDescriptor, normalize_username},
    protocol::{KIND_DEVICE_SIGNED_OUT, PROTOCOL_VERSION},
};

fn denied(message: &str) -> CoreError {
    CoreError::InvalidInput(message.to_owned())
}

impl AppCore {
    /// Состояние аккаунта для интерфейса.
    pub(super) fn account_view(&self) -> Result<AccountView, CoreError> {
        let account = self.store.account()?;
        let state = match &account {
            Some(_) => "active",
            None if self.store.is_pristine()? => "none",
            None => "legacy",
        };
        let names = self.store.device_names()?;
        let me = &self.identity.public.device_id;
        let mut devices: Vec<AccountDeviceView> = self
            .store
            .device_list()?
            .map(|list| list.document.devices)
            .unwrap_or_default()
            .into_iter()
            .map(|device| {
                let name = names.get(&device.device_id);
                AccountDeviceView {
                    current: &device.device_id == me,
                    name: name
                        .map(|value| value.name.clone())
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or_else(|| format!("Устройство {}", super::short_id(&device.device_id))),
                    added_at_unix_milliseconds: name
                        .map(|value| value.added_at_unix_milliseconds)
                        .unwrap_or(device.created_at_unix_milliseconds),
                    device_id: device.device_id,
                }
            })
            .collect();
        if devices.is_empty() {
            devices.push(AccountDeviceView {
                device_id: me.clone(),
                name: names
                    .get(me)
                    .map(|value| value.name.clone())
                    .unwrap_or_else(|| "Это устройство".to_owned()),
                current: true,
                added_at_unix_milliseconds: self.identity.public.created_at_unix_milliseconds,
            });
        }
        devices.sort_by(|left, right| {
            right
                .current
                .cmp(&left.current)
                .then(right.added_at_unix_milliseconds.cmp(&left.added_at_unix_milliseconds))
        });
        Ok(AccountView {
            state: state.to_owned(),
            username: account
                .as_ref()
                .map(|value| value.username.clone())
                .unwrap_or_else(|| self.store.profile().map(|profile| profile.username).unwrap_or_default()),
            node: account.as_ref().map(|value| value.base_url.clone()).unwrap_or_default(),
            recovery_key: account.as_ref().and_then(|value| value.pending_recovery_key.clone()),
            username_conflict: account.as_ref().is_some_and(|value| value.username_conflict),
            notice: self.account_notice.clone(),
            devices,
        })
    }

    pub(super) fn require_account(&self) -> Result<LocalAccount, CoreError> {
        self.store
            .account()?
            .ok_or_else(|| denied("Сначала войдите в аккаунт или создайте его"))
    }

    /// Проверка username до регистрации или смены: занят ли он на текущем Node.
    pub(super) fn check_username(&mut self, username: &str) -> Result<serde_json::Value, CoreError> {
        let username = normalize_username(username)?;
        let node = self.require_node()?;
        let available = self
            .network
            .username_available(&node, &username, &self.identity.public.user_id)?;
        Ok(json!({ "username": username, "available": available }))
    }

    /// Регистрация. Если на устройстве уже есть переписка (аккаунт создаётся для существующего
    /// профиля), она сохраняется — аккаунт просто начинает её защищать.
    pub(super) fn register_account(
        &mut self,
        username: &str,
        display_name: &str,
        password: &str,
        device_name: &str,
    ) -> Result<String, CoreError> {
        if self.store.account()?.is_some() {
            return Err(denied("Аккаунт уже создан"));
        }
        account::check_password(password)?;
        let username = normalize_username(username)?;
        let mut profile = self.store.profile()?;
        let display_name = display_name.trim();
        if !display_name.is_empty() {
            if display_name.chars().count() > 64 {
                return Err(denied("Видимое имя: 1–64 символа"));
            }
            profile.display_name = display_name.to_owned();
        }
        if profile.display_name.trim().is_empty() {
            return Err(denied("Укажите видимое имя"));
        }
        let Some(identity_key) = self.identity.identity_private_key().map(str::to_owned) else {
            return Err(denied(
                "Это устройство привязано старым способом и не хранит ключ личности. Создайте аккаунт на основном устройстве, а здесь войдите в него.",
            ));
        };
        let node = self.require_node().inspect_err(|_| self.online = false)?;
        self.online = true;
        let me = self.me();
        if !self.network.username_available(&node, &username, &me)? {
            return Err(CoreError::UsernameTaken(username));
        }

        let account_key = account::random_bytes::<32>();
        let salt = account::random_salt();
        let password_keys = account::derive_password(password, &salt)?;
        let recovery_raw = account::new_recovery_key();
        let recovery_keys = account::derive_recovery(&recovery_raw);
        let access_token = account::random_token();
        let credentials = ServerCredentials {
            login_lookup: account::login_lookup(&username),
            recovery_lookup: recovery_keys.lookup.clone(),
            password_salt: salt,
            password_verifier: account::verifier(&password_keys.auth),
            recovery_verifier: account::verifier(&recovery_keys.auth),
            access_verifier: account::verifier(&access_token),
            password_wrapped_key: account::wrap_key(&password_keys.wrap, PURPOSE_PASSWORD_WRAP, &account_key)?,
            recovery_wrapped_key: account::wrap_key(&recovery_keys.wrap, PURPOSE_RECOVERY_WRAP, &account_key)?,
        };
        let now = chrono::Utc::now().timestamp_millis();
        let vault = VaultContent {
            version: 1,
            user_id: me.clone(),
            identity_private_key: identity_key,
            access_token: access_token.clone(),
            created_at_unix_milliseconds: now,
            username: username.clone(),
            credentials: credentials.clone(),
        };
        let sealed = account::seal_vault(&account_key, &vault)?;
        let proof = account::account_proof(
            &credentials.login_lookup,
            &credentials.recovery_lookup,
            node.registration_pow_bits,
        );
        let account_id = self
            .network
            .create_account(&node.base_url, &credentials, &sealed, &proof)?;

        let recovery_text = account::format_recovery_key(&recovery_raw);
        profile.username = username.clone();
        self.store.save_profile(&profile)?;
        self.store.save_account(&LocalAccount {
            account_id,
            base_url: node.base_url.clone(),
            username,
            account_key: encode_key(&account_key),
            access_token,
            vault_version: 1,
            snapshot_version: 0,
            snapshot_uploaded_at_unix_milliseconds: 0,
            snapshot_seq: 0,
            sync_seq: self.store.sync_max_seq()?,
            known_devices: self.other_devices()?,
            pending_recovery_key: Some(recovery_text.clone()),
            username_conflict: false,
            snapshot_requested: true,
            last_snapshot_check_unix_milliseconds: now,
        })?;
        self.name_this_device(device_name)?;
        self.account_notice = None;

        // Профиль и username — в directory, чтобы собеседники нашли нового пользователя.
        if let Err(error) = self.publish_directory(&profile) {
            self.note_publish_error(&error);
        }
        if let Err(error) = self.upload_snapshot() {
            self.status = format!("Аккаунт создан, снимок данных выложим позже: {error}");
        } else {
            self.status = "Аккаунт создан".to_owned();
        }
        Ok(recovery_text)
    }

    /// Вход по username и паролю.
    pub(super) fn login_account(
        &mut self,
        username: &str,
        password: &str,
        device_name: &str,
        discard_local: bool,
    ) -> Result<(), CoreError> {
        self.ensure_can_join(discard_local)?;
        if password.is_empty() {
            return Err(denied("Введите пароль"));
        }
        let username = normalize_username(username)?;
        let node = self.require_node().inspect_err(|_| self.online = false)?;
        self.online = true;
        let lookup = account::login_lookup(&username);
        let salt = self.network.account_salt(&node.base_url, &lookup)?;
        let keys = account::derive_password(password, &salt)?;
        let secrets = self.network.login_account(&node.base_url, &lookup, &keys.auth)?;
        let account_key = account::unwrap_key(&keys.wrap, PURPOSE_PASSWORD_WRAP, &secrets.wrapped_key)
            .map_err(|_| denied("Неверный логин или пароль"))?;
        let vault = account::open_vault(&account_key, &secrets.vault)?;
        self.join_account(&node, account_key, vault, &secrets, &username, device_name, None)
    }

    /// Восстановление по ключу: новый пароль и новый ключ восстановления (использованный ключ
    /// мог попасться кому-то на глаза). Возвращает новый ключ.
    pub(super) fn recover_account(
        &mut self,
        recovery_key: &str,
        new_password: &str,
        device_name: &str,
        discard_local: bool,
    ) -> Result<String, CoreError> {
        account::check_password(new_password)?;
        let raw = account::parse_recovery_key(recovery_key)?;
        let recovery = account::derive_recovery(&raw);
        let node = self.require_node().inspect_err(|_| self.online = false)?;
        self.online = true;
        let secrets = self
            .network
            .recover_account(&node.base_url, &recovery.lookup, &recovery.auth)?;
        let account_key = account::unwrap_key(&recovery.wrap, PURPOSE_RECOVERY_WRAP, &secrets.wrapped_key)
            .map_err(|_| denied("Ключ восстановления не подошёл"))?;
        let mut vault = account::open_vault(&account_key, &secrets.vault)?;
        let current = self
            .store
            .account()?
            .is_some_and(|local| local.account_id == secrets.account_id);
        if !current {
            self.ensure_can_join(discard_local)?;
        }

        let salt = account::random_salt();
        let password_keys = account::derive_password(new_password, &salt)?;
        let new_raw = account::new_recovery_key();
        let new_recovery = account::derive_recovery(&new_raw);
        vault.credentials.password_salt = salt;
        vault.credentials.password_verifier = account::verifier(&password_keys.auth);
        vault.credentials.password_wrapped_key =
            account::wrap_key(&password_keys.wrap, PURPOSE_PASSWORD_WRAP, &account_key)?;
        vault.credentials.recovery_lookup = new_recovery.lookup.clone();
        vault.credentials.recovery_verifier = account::verifier(&new_recovery.auth);
        vault.credentials.recovery_wrapped_key =
            account::wrap_key(&new_recovery.wrap, PURPOSE_RECOVERY_WRAP, &account_key)?;
        self.network.put_account_credentials(
            &node.base_url,
            &secrets.account_id,
            &vault.access_token,
            &credentials_change(&vault.credentials, true, true),
        )?;
        let vault_version = self.write_vault(
            &node.base_url,
            &secrets.account_id,
            &vault.access_token,
            &account_key,
            &vault,
            secrets.vault_version,
        )?;
        let recovery_text = account::format_recovery_key(&new_raw);
        let username = vault.username.clone();
        if current {
            let mut local = self.require_account()?;
            local.vault_version = vault_version;
            local.pending_recovery_key = Some(recovery_text.clone());
            self.store.save_account(&local)?;
            self.status = "Пароль изменён".to_owned();
        } else {
            let secrets = AccountSecretsWire {
                vault_version,
                ..secrets
            };
            self.join_account(
                &node,
                account_key,
                vault,
                &secrets,
                &username,
                device_name,
                Some(recovery_text.clone()),
            )?;
        }
        Ok(recovery_text)
    }

    /// На устройстве с чужой перепиской войти можно только с её удалением — и только явно.
    fn ensure_can_join(&self, discard_local: bool) -> Result<(), CoreError> {
        if self.store.account()?.is_some() {
            return Err(denied("Вы уже вошли в аккаунт. Чтобы войти в другой, сначала выйдите."));
        }
        if !discard_local && !self.store.is_pristine()? {
            return Err(denied(
                "На этом устройстве есть переписка без аккаунта. Вход в другой аккаунт удалит её — подтвердите вход.",
            ));
        }
        Ok(())
    }

    /// Устройство становится частью аккаунта: свои ключи устройства, сертификат от ключа
    /// личности из сейфа, снимок данных и место в подписанном списке устройств.
    #[allow(clippy::too_many_arguments)]
    fn join_account(
        &mut self,
        node: &NodeDescriptor,
        account_key: [u8; 32],
        vault: VaultContent,
        secrets: &AccountSecretsWire,
        username: &str,
        device_name: &str,
        pending_recovery_key: Option<String>,
    ) -> Result<(), CoreError> {
        let identity = StoredIdentity::for_account(&vault.identity_private_key)?;
        if identity.public.user_id != vault.user_id {
            return Err(CoreError::Crypto(
                "Ключ личности в сейфе не совпадает с аккаунтом".to_owned(),
            ));
        }
        self.sign_out_local(None)?;
        self.store.replace_identity(&identity)?;
        self.identity = identity;
        let now = chrono::Utc::now().timestamp_millis();
        let mut local = LocalAccount {
            account_id: secrets.account_id.clone(),
            base_url: node.base_url.clone(),
            username: if username.is_empty() { vault.username.clone() } else { username.to_owned() },
            account_key: encode_key(&account_key),
            access_token: vault.access_token.clone(),
            vault_version: secrets.vault_version,
            snapshot_version: 0,
            snapshot_uploaded_at_unix_milliseconds: now,
            snapshot_seq: 0,
            sync_seq: 0,
            known_devices: Vec::new(),
            pending_recovery_key,
            username_conflict: false,
            snapshot_requested: false,
            last_snapshot_check_unix_milliseconds: now,
        };
        self.store.save_account(&local)?;
        if secrets.snapshot_version > 0
            && let Err(error) = self.merge_remote_snapshot(&mut local)
        {
            self.status = format!("История загрузится позже: {error}");
        }
        local.sync_seq = self.store.sync_max_seq()?;
        self.store.save_account(&local)?;
        // Профиль из снимка может прийти без username, если снимка ещё нет.
        let mut profile = self.store.profile()?;
        if profile.username.is_empty() {
            profile.username = local.username.clone();
            self.store.quietly(|| self.store.save_profile(&profile))?;
        }

        // Своё место в списке устройств: сначала берём опубликованный список, потом добавляемся.
        if let Ok(Some(remote)) = self.network.routing(node, &vault.user_id)
            && remote.verify()
        {
            self.adopt_device_list(&remote.descriptor.device_list)?;
            self.store.save_peer_routing(&remote)?;
        }
        self.ensure_transport(node)?;
        local.known_devices = self.other_devices()?;
        self.store.save_account(&local)?;
        self.name_this_device(device_name)?;
        self.announce_device()?;
        self.deliver_now();
        self.account_notice = None;
        self.status = "Вы вошли в аккаунт".to_owned();
        Ok(())
    }

    /// Смена пароля. Нужен текущий пароль: устройство в чужих руках не должно менять его молча.
    pub(super) fn change_password(&mut self, old_password: &str, new_password: &str) -> Result<(), CoreError> {
        account::check_password(new_password)?;
        let mut local = self.require_account()?;
        let account_key = local.key()?;
        let (mut vault, version) = self.read_vault(&local, &account_key)?;
        verify_password(&vault, old_password)?;
        let salt = account::random_salt();
        let keys = account::derive_password(new_password, &salt)?;
        vault.credentials.password_salt = salt;
        vault.credentials.password_verifier = account::verifier(&keys.auth);
        vault.credentials.password_wrapped_key =
            account::wrap_key(&keys.wrap, PURPOSE_PASSWORD_WRAP, &account_key)?;
        self.network.put_account_credentials(
            &local.base_url,
            &local.account_id,
            &local.access_token,
            &credentials_change(&vault.credentials, true, false),
        )?;
        local.vault_version = self.write_vault(
            &local.base_url,
            &local.account_id,
            &local.access_token,
            &account_key,
            &vault,
            version,
        )?;
        self.store.save_account(&local)?;
        self.status = "Пароль изменён".to_owned();
        Ok(())
    }

    /// Новый ключ восстановления; старый перестаёт действовать.
    pub(super) fn regenerate_recovery_key(&mut self, password: &str) -> Result<String, CoreError> {
        let mut local = self.require_account()?;
        let account_key = local.key()?;
        let (mut vault, version) = self.read_vault(&local, &account_key)?;
        verify_password(&vault, password)?;
        let raw = account::new_recovery_key();
        let recovery = account::derive_recovery(&raw);
        vault.credentials.recovery_lookup = recovery.lookup.clone();
        vault.credentials.recovery_verifier = account::verifier(&recovery.auth);
        vault.credentials.recovery_wrapped_key =
            account::wrap_key(&recovery.wrap, PURPOSE_RECOVERY_WRAP, &account_key)?;
        self.network.put_account_credentials(
            &local.base_url,
            &local.account_id,
            &local.access_token,
            &credentials_change(&vault.credentials, false, true),
        )?;
        local.vault_version = self.write_vault(
            &local.base_url,
            &local.account_id,
            &local.access_token,
            &account_key,
            &vault,
            version,
        )?;
        let text = account::format_recovery_key(&raw);
        local.pending_recovery_key = Some(text.clone());
        self.store.save_account(&local)?;
        self.status = "Создан новый ключ восстановления".to_owned();
        Ok(text)
    }

    /// Смена username: логин аккаунта меняется вместе с ним.
    pub(super) fn change_account_username(&mut self, username: &str) -> Result<(), CoreError> {
        let mut local = self.require_account()?;
        if local.username == username && !local.username_conflict {
            return Ok(());
        }
        let node = self.require_node()?;
        if !self
            .network
            .username_available(&node, username, &self.identity.public.user_id)?
        {
            return Err(CoreError::UsernameTaken(username.to_owned()));
        }
        // Аккаунт ещё не перенесён на текущий Node — логин сменится при переносе.
        if local.base_url.trim_end_matches('/') == node.base_url.trim_end_matches('/') {
            let account_key = local.key()?;
            let (mut vault, version) = self.read_vault(&local, &account_key)?;
            vault.username = username.to_owned();
            vault.credentials.login_lookup = account::login_lookup(username);
            self.network.put_account_credentials(
                &local.base_url,
                &local.account_id,
                &local.access_token,
                &json!({ "loginLookup": vault.credentials.login_lookup }),
            )?;
            local.vault_version = self.write_vault(
                &local.base_url,
                &local.account_id,
                &local.access_token,
                &account_key,
                &vault,
                version,
            )?;
        }
        local.username = username.to_owned();
        local.username_conflict = false;
        self.store.save_account(&local)?;
        Ok(())
    }

    /// Перенос аккаунта на Node, к которому теперь подключено устройство. Сейф и ключи те же,
    /// меняется только место хранения; на прежнем Node запись остаётся для других устройств.
    pub(super) fn migrate_account(&mut self, node: &NodeDescriptor) -> Result<(), CoreError> {
        let mut local = self.require_account()?;
        let me = self.me();
        if !self.network.username_available(node, &local.username, &me)? {
            if !local.username_conflict {
                local.username_conflict = true;
                self.store.save_account(&local)?;
            }
            return Err(CoreError::UsernameTaken(local.username.clone()));
        }
        let account_key = local.key()?;
        let (mut vault, _) = self.read_vault(&local, &account_key)?;
        vault.username = local.username.clone();
        vault.credentials.login_lookup = account::login_lookup(&local.username);
        let sealed = account::seal_vault(&account_key, &vault)?;
        let proof = account::account_proof(
            &vault.credentials.login_lookup,
            &vault.credentials.recovery_lookup,
            node.registration_pow_bits,
        );
        let account_id = self
            .network
            .create_account(&node.base_url, &vault.credentials, &sealed, &proof)?;
        local.account_id = account_id;
        local.base_url = node.base_url.clone();
        local.vault_version = 1;
        local.snapshot_version = 0;
        local.snapshot_requested = true;
        local.username_conflict = false;
        self.store.save_account(&local)?;
        self.status = format!("Аккаунт перенесён на {}", node.name);
        Ok(())
    }

    /// Выход: другие устройства убирают это из списка, а здесь стирается всё.
    pub(super) fn logout(&mut self) -> Result<(), CoreError> {
        if self.store.account()?.is_some() && !self.other_devices()?.is_empty() {
            let me = self.me();
            let _ = self.queue_event(
                &me,
                &format!("evt1-{}", super::random_hex(16)),
                KIND_DEVICE_SIGNED_OUT,
                &super::sync::DevicePayload {
                    version: PROTOCOL_VERSION,
                    device_id: self.identity.public.device_id.clone(),
                },
            );
            self.deliver_now();
        }
        self.sign_out_local(None)?;
        self.status = "Вы вышли из аккаунта".to_owned();
        Ok(())
    }

    /// Стереть с устройства всё, кроме адреса Node, и начать с чистой временной личности.
    pub(super) fn sign_out_local(&mut self, notice: Option<&str>) -> Result<(), CoreError> {
        calls::with_call(&self.call_slot.clone(), |slot| {
            if let Some(call) = slot.as_mut() {
                call.finish("hangup", None);
            }
            *slot = None;
        });
        if let Ok(mut watch) = self.watch.lock() {
            *watch = None;
        }
        for job in self.media_jobs.values() {
            job.progress.cancel();
        }
        self.media_jobs.clear();
        self.store.wipe()?;
        self.identity = self.store.load_or_create_identity()?;
        self.selected_contact = None;
        self.selected_thread = None;
        self.search_query.clear();
        self.last_device_refresh_unix_milliseconds = 0;
        self.account_notice = notice.map(str::to_owned);
        if let Some(notice) = notice {
            self.status = notice.to_owned();
        }
        Ok(())
    }

    /// Имя этого устройства в списке сеансов.
    pub(super) fn name_this_device(&mut self, name: &str) -> Result<(), CoreError> {
        let name: String = name.trim().chars().take(64).collect();
        let name = if name.is_empty() { "Устройство".to_owned() } else { name };
        let device_id = self.identity.public.device_id.clone();
        let added = self
            .store
            .device_names()?
            .get(&device_id)
            .map(|value| value.added_at_unix_milliseconds)
            .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
        self.store.save_device_name(
            &device_id,
            &DeviceName {
                name,
                added_at_unix_milliseconds: added,
            },
        )
    }

    /// Ошибка публикации профиля: занятый username — это не сбой связи, а повод выбрать другой.
    pub(super) fn note_publish_error(&mut self, error: &CoreError) {
        if matches!(error, CoreError::UsernameTaken(_)) {
            if let Ok(Some(mut local)) = self.store.account() {
                local.username_conflict = true;
                let _ = self.store.save_account(&local);
            }
        }
        self.status = error.to_string();
    }

    fn read_vault(&self, local: &LocalAccount, account_key: &[u8; 32]) -> Result<(VaultContent, i64), CoreError> {
        let wire = self
            .network
            .account_vault(&local.base_url, &local.account_id, &local.access_token)?;
        Ok((account::open_vault(account_key, &wire.vault)?, wire.version))
    }

    /// Записать сейф поверх версии `version`. Если его успели изменить с другого устройства —
    /// значит, изменения могут потеряться: просим повторить.
    fn write_vault(
        &self,
        base_url: &str,
        account_id: &str,
        access: &str,
        account_key: &[u8; 32],
        vault: &VaultContent,
        version: i64,
    ) -> Result<i64, CoreError> {
        let sealed = account::seal_vault(account_key, vault)?;
        self.network
            .put_account_vault(base_url, account_id, access, &sealed, version)?
            .ok_or_else(|| denied("Данные аккаунта только что изменили с другого устройства. Повторите действие."))
    }
}

/// Проверка пароля без обращения к Node: сейф хранит хеш ключа входа.
fn verify_password(vault: &VaultContent, password: &str) -> Result<(), CoreError> {
    let keys = account::derive_password(password, &vault.credentials.password_salt)?;
    if account::verifier(&keys.auth) != vault.credentials.password_verifier {
        return Err(denied("Текущий пароль введён неверно"));
    }
    Ok(())
}

fn credentials_change(credentials: &ServerCredentials, password: bool, recovery: bool) -> serde_json::Value {
    let mut value = json!({});
    if password {
        value["passwordSalt"] = json!(credentials.password_salt);
        value["passwordVerifier"] = json!(credentials.password_verifier);
        value["passwordWrappedKey"] = json!(credentials.password_wrapped_key);
    }
    if recovery {
        value["recoveryLookup"] = json!(credentials.recovery_lookup);
        value["recoveryVerifier"] = json!(credentials.recovery_verifier);
        value["recoveryWrappedKey"] = json!(credentials.recovery_wrapped_key);
    }
    value
}
