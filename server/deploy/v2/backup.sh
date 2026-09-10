#!/usr/bin/env bash
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir"
backup_dir="${1:-$script_dir/backups}"
mkdir -p "$backup_dir"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
docker compose exec -T postgres pg_dump -U turattext -d turattext -Fc > "$backup_dir/postgres-$stamp.dump"
docker compose cp node:/app/data/node-identity-v2.json "$backup_dir/node-identity-$stamp.json"
chmod 600 "$backup_dir"/*
echo "Backup written to $backup_dir"

