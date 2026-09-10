# Развёртывание TuratText v2 Node на VPS

Архив `artifacts/TuratText-VPS-Node.zip` самодостаточен. На VPS не нужны Gradle, .NET или исходный
код — JAR уже собран. Прежний центральный IP не используется.

## До установки

1. Создайте A-запись домена на IPv4 VPS и, только если IPv6 настроен, AAAA-запись.
2. Разрешите входящие TCP 80/443 и UDP 443. UDP нужен для HTTP/3, но его блокировка не мешает HTTPS.
3. Установите Docker Engine и плагин Docker Compose v2.
4. Скопируйте архив на VPS и проверьте SHA-256 по `SHA256SUMS.txt`.

## Установка

```bash
unzip TuratText-VPS-Node.zip
cd v2
chmod +x install.sh backup.sh restore.sh
./install.sh node.example.org admin@example.org "My TuratText Node"
```

`install.sh`:

- проверяет Docker Compose и наличие готового JAR;
- генерирует PostgreSQL/JWT secrets и сохраняет `.env` с правами `0600`;
- запускает PostgreSQL 17, непривилегированный Java 21 Node и Caddy;
- получает публичный TLS-сертификат;
- включает v2-only, PoW, capability mailboxes и корректный forwarded-IP rate limiting;
- ждёт `https://DOMAIN/v2/health` до готовности.

Проверка:

```bash
curl -fsS https://node.example.org/v2/health
curl -fsS https://node.example.org/v2/node-descriptor
docker compose ps
docker compose logs --tail=100 node caddy
```

Legacy URL должен возвращать 404:

```bash
curl -o /dev/null -s -w '%{http_code}\n' https://node.example.org/api/servers/health
```

## Подключение клиентов

Официальный Windows/Android клиент автоматически подключается к
`https://turattext.rplacefree.store`, проверяет закреплённый Ed25519 `NodeID`, регистрирует
capabilities и разделённый prekey batch и публикует подписанный routing record. Другие независимые
Nodes при необходимости добавляются в раскрываемом разделе «Сеть и приватность».

## Backup и восстановление Node

```bash
./backup.sh
```

Будут сохранены PostgreSQL custom dump и node identity. Node identity нужна, чтобы после аварии
дескриптор сохранил тот же `NodeID`; держите backup зашифрованным и вне VPS.

Восстановление PostgreSQL и постоянного `NodeID` одной командой:

```bash
./restore.sh \
  backups/postgres-YYYYMMDDTHHMMSSZ.dump \
  backups/node-identity-YYYYMMDDTHHMMSSZ.json
```

Скрипт останавливает Node, восстанавливает дамп с `--exit-on-error`, безопасно кладёт identity в
volume `node-data` с владельцем непривилегированного процесса и ждёт публичный health check.

## Обновления

Caddy раздаёт `updates/stable/manifest.json`, Windows ZIP и Android APK из каталога `updates/`.
Manifest текущего релиза уже подписан 2 из 3 offline Ed25519 ключей и использует относительные URL,
поэтому работает на любом домене.

Для следующего релиза увеличьте `sequence`, пересоберите пакеты и выполните на offline-машине:

```powershell
dotnet run --project tools/UpdateAuthority/UpdateAuthority.csproj -c Release -- `
  sign release-secrets/update-authority release-spec.json manifest.json
```

На VPS копируются только manifest и пакеты. `.key`, Android keystore и пароли на VPS копировать
нельзя.

## Эксплуатация

```bash
docker compose pull
docker compose up -d --build
docker compose restart node
docker compose logs -f --tail=100 node
```

Volumes `postgres-data`, `node-data`, `caddy-data` и `.env` нельзя удалять при обычном обновлении.
Для второго независимого Node используйте другой VPS/домен: он обязан создать собственный NodeID,
а не копировать identity первого Node.
