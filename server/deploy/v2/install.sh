#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <node-domain> <acme-email> [node-name]" >&2
  exit 2
fi
if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
  echo "Docker Engine with Compose v2 is required." >&2
  exit 1
fi

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir"
if [[ ! -s turattext-server.jar ]]; then
  echo "turattext-server.jar is missing from the deployment directory." >&2
  exit 1
fi
umask 077
domain="$1"
email="$2"
node_name="${3:-TuratText Node}"
postgres_password="$(openssl rand -base64 36 | tr -d '\n')"
jwt_secret="$(openssl rand -base64 48 | tr -d '\n')"
temporary_env="$(mktemp "$script_dir/.env.XXXXXX")"
trap 'rm -f "$temporary_env"' EXIT
{
  printf 'TURATTEXT_DOMAIN=%s\n' "$domain"
  printf 'ACME_EMAIL=%s\n' "$email"
  printf 'TURATTEXT_SERVER_NAME=%s\n' "$node_name"
  printf 'POSTGRES_PASSWORD=%s\n' "$postgres_password"
  printf 'JWT_SECRET=%s\n' "$jwt_secret"
  printf 'TURATTEXT_V2_REGISTRATION_POW_BITS=18\n'
  printf 'TURATTEXT_V2_ENVELOPE_POW_BITS=10\n'
  printf 'TURATTEXT_V2_CONTACT_POW_BITS=18\n'
  printf 'TURATTEXT_V2_CONTACT_MAX_ENVELOPE_BYTES=65536\n'
  printf 'TURATTEXT_V2_CONTACT_MAX_ENVELOPES=64\n'
  printf 'TURATTEXT_V2_REQUESTS_PER_MINUTE=1200\n'
} > "$temporary_env"
mv -f "$temporary_env" .env
trap - EXIT
chmod 600 .env

docker compose up -d --build
for _ in $(seq 1 90); do
  if curl -fsS "https://$domain/v2/health" >/dev/null 2>&1; then
    echo "TuratText v2 Node is ready: https://$domain"
    curl -fsS "https://$domain/v2/node-descriptor"
    printf '\n'
    exit 0
  fi
  sleep 2
done
docker compose logs --tail=150 node caddy >&2
echo "Node did not become healthy. Check DNS A/AAAA records and ports 80/443." >&2
exit 1
