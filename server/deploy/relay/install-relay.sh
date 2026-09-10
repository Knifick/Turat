#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 2 ]]; then
  echo "Usage: $0 <public-relay-host-or-ip> <target-node-domain> [listen-port]" >&2
  exit 2
fi
if ! command -v docker >/dev/null 2>&1 || ! docker compose version >/dev/null 2>&1; then
  echo "Docker Engine with Compose v2 is required." >&2
  exit 1
fi
for command in openssl python3; do
  command -v "$command" >/dev/null 2>&1 || { echo "$command is required" >&2; exit 1; }
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir"
connect_host="$1"
target_host="$2"
listen_port="${3:-443}"
umask 077
if [[ ! -f relay-private.pem ]]; then
  openssl genpkey -algorithm Ed25519 -out relay-private.pem
fi
cat > haproxy.cfg <<EOF
global
    maxconn 4096
    log stdout format raw local0
defaults
    mode tcp
    timeout connect 8s
    timeout client 60s
    timeout server 60s
frontend turattext_relay
    bind :$listen_port
    default_backend fixed_turattext_node
backend fixed_turattext_node
    server node $target_host:443 check resolvers dns init-addr libc,none
resolvers dns
    nameserver resolver1 1.1.1.1:53
    resolve_retries 3
    timeout resolve 2s
    timeout retry 2s
EOF
printf 'RELAY_PORT=%s\n' "$listen_port" > .env
docker compose up -d
python3 create-relay-descriptor.py relay-private.pem "$connect_host" "$listen_port" "$target_host" 443 relay-descriptor.json
chmod 600 relay-private.pem
chmod 644 relay-descriptor.json
echo "Relay is ready. Distribute relay-descriptor.json through a trusted/social channel."

