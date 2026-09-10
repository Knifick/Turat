# Turat 3.0 native local-first

Turat больше не требует доступности прежнего центрального сервера. Криптографическая identity,
история, outbox и контакты принадлежат клиенту; VPS запускает заменяемый v2 Node для mailbox,
prekeys, routing, transparency и зашифрованных blob-объектов.

Официальный клиент автоматически использует `https://turattext.rplacefree.store` как основной
bootstrap Node и проверяет закреплённый `NodeID`. Выбор сервера при обычном запуске не требуется;
другие независимые Nodes можно добавить в разделе «Сеть и приватность».

## Готовые пакеты

- `artifacts/Turat-win-x64.zip` — self-contained Windows x64 клиент.
- `artifacts/Turat.apk` — Android 6+ APK для arm/arm64, подписанный постоянным release-key.
- `artifacts/TuratText-VPS-Node.zip` — самодостаточный VPS Node: JAR, PostgreSQL, Caddy,
  HTTPS/HTTP2/HTTP3, миграции, backup и подписанные обновления.
- `artifacts/SHA256SUMS.txt` — SHA-256 всех пакетов.

Offline-ключи обновлений и Android находятся в `release-secrets/` и намеренно исключены из Git и
VPS-архива. Сохраните этот каталог в двух независимых зашифрованных offline-копиях.

## Что реализовано

- производный от identity public key `UserID`, отдельные `DeviceID` и сертификаты устройств;
- link-пакеты без копирования master identity key, root-подписанный отзыв устройств и защита
  Directory от отката `DeviceList`;
- подписанный зашифрованный SQLite event log, persistent outbox, at-least-once доставка и dedup;
- text/edit/delete/reaction, delivery/read receipts и pending contact requests;
- public contact inbox с отдельным capability, небольшими text-only запросами и proof-of-work;
- несколько Mailbox Nodes, randomized fixed-size envelopes и приватные reply capabilities;
- X25519 + ML-KEM-768 initial agreement, ratchet sessions и skipped-message keys;
- непересекающиеся prekey batches для разных Nodes;
- зашифрованные chunked attachments, несколько Blob Nodes и capability-ссылки внутри E2EE;
- подписанные Node/Relay/Routing/Discovery descriptors, social bridge bundles и relay fallback;
- HTTP/2 с переходом на HTTP/3/QUIC через Caddy, fixed-target TLS relay и relay-first privacy mode;
- fast/balanced/high metadata modes с padding, batching через outbox и случайной задержкой;
- username как изменяемый identity-подписанный указатель с явным обнаружением конфликтов;
- append-only transparency operations, Merkle checkpoints и client-side pinning;
- encrypted backup, перенос истории, физические `.ttenv` mesh-пакеты и периодический fetch без push;
- group epoch control plane с fork detection и out-of-order sender-key cache;
- 2-of-3 Ed25519 release manifests, rollback/equivocation pinning и несколько зеркал пакетов;
- production v2-only режим: legacy API возвращает 404.

## Интерфейс и привычные возможности мессенджера

UI обоих клиентов повторяет Telegram: Android — Telegram for Android (боковое меню, пузыри
с «хвостом» и временем внутри, круглая кнопка нового чата), Windows — Unigram (левый рельс
с профилем, список чатов, контекстные меню, собственная полоса заголовка).

Оформление выбирается в «Настройках → Тема»; выбор запоминается на устройстве. Доступны
одиннадцать тем: Ориджин (фирменная), Гранат, Обсидиан, Чёрный, Графит, Изумрудный, Океан,
Янтарь, Аметист и две светлые — Облачный и Пергамент.

- список чатов с превью последнего события, временем, галочками и счётчиком непрочитанных;
- закреплённые чаты, режим «без звука», отметка «непрочитано», очистка истории;
- черновики: незаконченное сообщение сохраняется и видно в списке чатов;
- ответы на сообщения с цитатой и переходом к оригиналу, пересылка в другой диалог;
- реакции, редактирование, удаление, выделение нескольких сообщений и копирование;
- разделители дат, группировка подряд идущих сообщений одного автора;
- глобальный поиск по чатам и по тексту всех сообщений;
- фотография профиля: изображение уменьшается до 256 px и публикуется вместе с профилем;
- публикация профиля и username в directory Node — иначе собеседники не найдут вас по `@username`;
- «последняя активность» (`/v2/presence`) — подписанная запись directory, по умолчанию выключена;
  включение и выключение управляются в «Конфиденциальности», выключение отзывает запись;
- связь с Node поддерживается сама: клиент подключается при запуске и обновляет диалоги в фоне,
  а кнопка синхронизации только ускоряет очередной цикл;
- Windows: Enter отправляет сообщение, Shift+Enter переносит строку, Esc снимает правку и ответ;
- Android: свайп вправо в диалоге возвращает к списку чатов, боковое меню открывается свайпом
  только из самого списка.

## Установка Node на VPS

Понадобятся домен с A/AAAA-записью на VPS, открытые TCP 80/443 и UDP 443, Docker Engine и
Docker Compose v2.

```bash
unzip TuratText-VPS-Node.zip
cd v2
chmod +x install.sh backup.sh restore.sh
./install.sh node.example.org admin@example.org "My Turat Node"
```

Скрипт создаёт секреты с `0600`, собирает непривилегированный контейнер, запускает PostgreSQL и
Caddy, получает TLS-сертификат и ждёт успешный `/v2/health`. Подробности: `docs/vps-bootstrap.md`.

После установки просто откройте официальный клиент: основной Node подключается автоматически,
регистрирует mailbox/prekeys и восстанавливает синхронизацию после офлайна. Старый IP не нужен.

## Нативные клиенты

- `client/core` — единое Rust-ядро: identity, зашифрованный SQLite/WAL, local-first состояние,
  контакты, сообщения, вложения, backup/device-link/portable bundles, проверка Node descriptor и
  directory-записей. Публичная граница — JSON-команды поверх C ABI; Android использует JNI,
  Windows — P/Invoke.
- `client/android` — Kotlin-приложение с Jetpack Compose и Material 3. Ключ локального vault
  оборачивается Android Keystore; Rust `.so` собирается для `arm64-v8a` и `armeabi-v7a`.
- `client/windows` — C# + XAML + WinUI 3. Ключ vault защищён DPAPI текущего пользователя; Rust
  DLL встраивается в self-contained single-file EXE.

Старые `client/TuratText.Client` и `client/TuratText.Android` не входят в production-сборку и
оставлены только как источник форматов v2 для миграции существующих локальных данных.

## Сборка и проверки

Полная release-сборка обеих платформ, включая Rust-тесты, Android release-подпись и проверку APK:

```powershell
.\scripts\build-clients.ps1
```

Нужны stable Rust с `cargo-ndk`, Visual Studio Build Tools (MSVC), .NET 8 SDK, Android SDK 36,
NDK и JDK 17+. Gradle 9.6.1 закреплён wrapper-ом в Android-проекте.

Проверки серверной части:

```powershell
cd server
gradle clean test bootJar

cd ..
$env:TURATTEXT_SMOKE_NODE='http://127.0.0.1:18080'
dotnet run --project tools/LocalFirstSmoke/LocalFirstSmoke.csproj -c Release
```

Android и Windows собираются из общего Rust core. Сервер использует Java 21,
Spring Boot 3.5.16 и PostgreSQL 17.

## Граница безопасности

Реализация функционально завершает v2 vertical slice, но ещё не проходила независимый
криптографический аудит. Ratchet и group-epoch слой являются собственной реализацией, а не
сертифицированной сборкой Signal Protocol/MLS. До публичного high-risk запуска необходимы внешний
аудит, fuzzing подписываемых форматов и нагрузочные испытания нескольких независимых Nodes.
