#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "Usage: $0 <postgres.dump> <node-identity.json>" >&2
  exit 2
fi
if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
  echo "Docker Engine with Compose v2 is required." >&2
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir"
dump_file="$(realpath -- "$1")"
identity_file="$(realpath -- "$2")"

if [[ ! -s "$dump_file" ]]; then
  echo "PostgreSQL dump is missing or empty: $dump_file" >&2
  exit 1
fi
if [[ ! -s "$identity_file" ]]; then
  echo "Node identity is missing or empty: $identity_file" >&2
  exit 1
fi
if [[ ! -s .env ]]; then
  echo ".env is missing. Install the Node before restoring a backup." >&2
  exit 1
fi

echo "Stopping TuratText Node for an offline restore..."
docker compose stop node
docker compose up -d postgres

echo "Restoring PostgreSQL from $dump_file..."
docker compose exec -T postgres \
  pg_restore -U turattext -d turattext --clean --if-exists --exit-on-error < "$dump_file"

echo "Restoring the persistent Node identity..."
docker compose run --rm --no-deps --user root \
  --volume "$identity_file:/restore/node-identity-v2.json:ro" \
  --entrypoint sh node \
  -c 'install -o 10001 -g 10001 -m 600 /restore/node-identity-v2.json /app/data/node-identity-v2.json'

docker compose up -d node caddy
domain="$(sed -n 's/^TURATTEXT_DOMAIN=//p' .env | tail -n 1)"
if [[ -n "$domain" ]]; then
  for _ in $(seq 1 90); do
    if curl -fsS "https://$domain/v2/health" >/dev/null 2>&1; then
      echo "Restore completed. TuratText v2 Node is ready: https://$domain"
      exit 0
    fi
    sleep 2
  done
fi

docker compose logs --tail=150 node caddy >&2
echo "Restore completed, but the public health check did not become ready." >&2
exit 1
