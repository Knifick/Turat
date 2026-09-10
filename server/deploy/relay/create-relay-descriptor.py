#!/usr/bin/env python3
import base64
import hashlib
import json
import pathlib
import struct
import subprocess
import sys
import tempfile
import time

if len(sys.argv) != 7:
    raise SystemExit("usage: create-relay-descriptor.py PRIVATE_PEM CONNECT_HOST CONNECT_PORT TARGET_HOST TARGET_PORT OUTPUT")

private_pem, connect_host, connect_port, target_host, target_port, output = sys.argv[1:]
connect_port = int(connect_port)
target_port = int(target_port)
public_der = subprocess.check_output([
    "openssl", "pkey", "-in", private_pem, "-pubout", "-outform", "DER"
])
public_key = base64.b64encode(public_der).decode("ascii")
relay_id = "ttr1-" + hashlib.sha256(public_der).hexdigest()
expires = int((time.time() + 180 * 24 * 3600) * 1000)
transports = ["tls-tcp"]

def text(value):
    raw = value.encode("utf-8")
    return struct.pack(">i", len(raw)) + raw

canonical = b"".join([
    text("TuratText.RelayDescriptor"),
    struct.pack(">i", 2),
    text(relay_id),
    text(connect_host),
    struct.pack(">i", connect_port),
    text(target_host),
    struct.pack(">i", target_port),
    text(public_key),
    struct.pack(">q", expires),
    struct.pack(">i", len(transports)),
    *[text(value) for value in transports],
])
with tempfile.NamedTemporaryFile(delete=False) as source:
    source.write(canonical)
    source_path = source.name
signature_path = source_path + ".sig"
try:
    subprocess.check_call([
        "openssl", "pkeyutl", "-sign", "-rawin", "-inkey", private_pem,
        "-in", source_path, "-out", signature_path
    ])
    signature = base64.b64encode(pathlib.Path(signature_path).read_bytes()).decode("ascii")
finally:
    pathlib.Path(source_path).unlink(missing_ok=True)
    pathlib.Path(signature_path).unlink(missing_ok=True)

descriptor = {
    "version": 2,
    "relayId": relay_id,
    "connectHost": connect_host,
    "connectPort": connect_port,
    "targetHost": target_host,
    "targetPort": target_port,
    "publicKey": public_key,
    "algorithm": "Ed25519",
    "expiresAtUnixMilliseconds": expires,
    "transports": transports,
    "signature": signature,
}
pathlib.Path(output).write_text(json.dumps(descriptor, indent=2), encoding="utf-8")
print(json.dumps(descriptor, indent=2))

