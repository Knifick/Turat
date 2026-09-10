# TuratText Architecture

TuratText состоит из независимого v2 Node и двух полностью нативных клиентов. Сервер не получает
plaintext сообщений или приватные identity-ключи.

## Client core

`client/core` — единственная реализация клиентской предметной логики. Rust crate собирается как
`cdylib` для Windows x64, Android arm64 и Android armv7. В ядре находятся:

- P-256 root/device identity и совместимые `tt1-*`, `ttd1-*` идентификаторы;
- AES-256-GCM encrypted SQLite event/contact store с WAL;
- local-first outbox, проекции сообщений, edit/delete/reaction/read state;
- profile/contact state, attachment vault, encrypted backups и device-link пакеты;
- `.ttenv` и `.ttbridge` импорт/экспорт;
- HTTPS Node discovery, закрепление NodeID и проверка подписей Node/username/profile.

ABI намеренно узкий: `create`, `invoke(JSON)` и `destroy`. Каждая команда возвращает новый
immutable snapshot, поэтому Kotlin и C# не реализуют правила предметной области повторно.

## Android

`client/android` — Kotlin + Jetpack Compose/Material 3. `MainActivity` содержит только Android
file-picker интеграцию, Compose UI и lifecycle glue. `NativeCore` вызывает Rust напрямую через JNI.
Vault key генерируется случайно и оборачивается AES-GCM ключом из Android Keystore.

Интерфейс адаптивный: на телефоне список и чат разделены навигацией, на широком экране показаны
одновременно. Реализованы onboarding, поиск/добавление контакта, чат, multi-select, реакции,
редактирование/удаление, вложения, профиль, Node/privacy settings, backup/device link и offline
bundle workflows. Legacy Android Views/layout XML не используются.

## Windows

`client/windows` — C# + XAML + WinUI 3 на stable Windows App SDK. C# отвечает за WinUI controls,
native file pickers, clipboard и отображение snapshot; вызовы Rust идут через source-generated
P/Invoke. Vault key защищается Windows DPAPI для текущего пользователя.

Release публикуется как self-contained single-file x64 EXE. Rust DLL и WinUI resources встраиваются
при публикации. `--core-smoke` проверяет ABI и загрузку embedded Rust core без запуска окна.

## Build pipeline

`scripts/build-clients.ps1` выполняет один воспроизводимый pipeline:

1. запускает Rust unit tests и собирает MSVC `turattext_core.dll`;
2. публикует C#/XAML WinUI 3 single-file EXE и запускает ABI smoke-test;
3. собирает Rust `.so` для двух Android ABI через NDK;
4. собирает Kotlin/Compose release APK, выполняет zipalign и подписывает offline release-keystore;
5. проверяет подпись APK и обновляет `artifacts/SHA256SUMS.txt`.

## Backend

`server` — Java 21/Spring Boot v2 Node. Он хранит opaque mailbox envelopes, prekeys, routing,
username/profile claims, transparency operations и encrypted blobs. PostgreSQL является durable
хранилищем Node, Caddy завершает TLS и рекламирует HTTP/3.

