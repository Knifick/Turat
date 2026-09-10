# Native client layout

| Каталог | Язык/UI | Назначение |
|---|---|---|
| `core` | Rust | Общее local-first ядро, crypto/storage/network и C ABI/JNI |
| `android` | Kotlin + Jetpack Compose | Нативный Android UI и Android Keystore/file-picker glue |
| `windows` | C# + XAML + WinUI 3 | Нативный Windows UI и DPAPI/file-picker/clipboard glue |

UI отправляет в ядро JSON с discriminator `command`. Ответ всегда имеет `ok`, опциональные `error`
и `value`, а также полный `snapshot`, пригодный для немедленного render. Долгие вызовы выполняются
в `Dispatchers.IO` на Android и `Task.Run` на Windows.

Примеры:

```json
{"command":"send_text","user_id":"tt1-…","text":"Привет"}
{"command":"react","event_ids":["evt1-…"],"reaction":"👍"}
{"command":"sync"}
```

Production-артефакты собираются только через `scripts/build-clients.ps1`. Старые Avalonia/.NET
проекты не являются зависимостями новых клиентов и сохранены временно для миграции форматов v2.

