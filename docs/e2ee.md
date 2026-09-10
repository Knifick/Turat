# End-to-End Encryption

## Real E2EE

In real E2EE, plaintext exists only on users' devices. The server receives public keys, encrypted messages and metadata, but never receives private keys or plaintext messages.

## MVP model

1. The C# client creates a private/public identity key pair.
2. The private key is stored locally and can be protected with a passphrase-derived key.
3. The public key is uploaded to the Java backend.
4. For a direct chat, the sender fetches the recipient public key.
5. The sender derives a shared secret using standard ECDH APIs.
6. The sender encrypts message text with an AEAD cipher.
7. The backend stores only `encryptedContent`, `nonce` and key metadata.
8. The recipient downloads the ciphertext and decrypts it locally.

This MVP is not a full Signal implementation. It does not yet provide Double Ratchet forward secrecy, skipped message key handling, multi-device sessions or signed prekeys.

## Production upgrades

- Signal Protocol / Double Ratchet.
- X3DH or another audited prekey handshake.
- Per-device keys and device verification.
- Safety number / QR verification.
- Key rotation and revocation UX.
- Secure OS keychain integration.
- Encrypted local database on the client.

## What is not E2EE

Server-side encryption means the server receives plaintext and encrypts it before saving. The server can still read messages.

Database encryption at rest protects files on disk, but the running server can still read plaintext. It is useful defense in depth, not E2EE.

