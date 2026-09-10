namespace TuratText.Client.LocalFirst;

public sealed record ProtocolIdentity(
    int Version,
    string UserId,
    string IdentityAlgorithm,
    string IdentityPublicKey,
    string DeviceId,
    string DeviceAlgorithm,
    string DevicePublicKey,
    long CreatedAtUnixMilliseconds,
    string DeviceCertificate);
