using TuratText.Client.LocalFirst;

namespace TuratText.Client.Crypto.V2;

public sealed record RatchetMessage(
    int Version,
    string SessionId,
    string SenderUserId,
    string SenderDeviceId,
    string RecipientUserId,
    string RecipientDeviceId,
    string RatchetPublicKey,
    int PreviousChainLength,
    int MessageNumber,
    string Nonce,
    string Ciphertext);

public sealed record InitialHandshakeHeader(
    int Version,
    string SessionId,
    ProtocolIdentity SenderIdentity,
    string RecipientUserId,
    string RecipientDeviceId,
    string SenderIdentityDhPublicKey,
    string SenderEphemeralPublicKey,
    string RecipientSignedPrekeyId,
    string RecipientPqPrekeyId,
    string? RecipientOneTimePrekeyId,
    string PqCiphertext,
    long CreatedAtUnixMilliseconds);

public sealed record InitialSessionEnvelope(
    InitialHandshakeHeader Header,
    string HeaderJson,
    string HeaderSignature,
    RatchetMessage Message);

public sealed record DecryptedRatchetMessage(
    string SessionId,
    string SenderUserId,
    string SenderDeviceId,
    byte[] Plaintext);
