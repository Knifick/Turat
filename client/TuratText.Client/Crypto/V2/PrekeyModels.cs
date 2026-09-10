using TuratText.Client.LocalFirst;

namespace TuratText.Client.Crypto.V2;

public sealed record DevicePrekeyDescriptor(
    int Version,
    string UserId,
    string DeviceId,
    string IdentityDhPublicKey,
    string SignedPrekeyId,
    string SignedPrekeyPublicKey,
    string PqPrekeyId,
    string PqPrekeyPublicKey,
    long Sequence,
    long ExpiresAtUnixMilliseconds);

public sealed record SignedDevicePrekey(
    DevicePrekeyDescriptor Descriptor,
    string DescriptorJson,
    string Signature);

public sealed record OneTimePrekeyPublic(
    string PrekeyId,
    string PublicKey,
    string Signature);

public sealed record PrekeyPublication(
    ProtocolIdentity Identity,
    SignedDevicePrekey SignedPrekey,
    IReadOnlyList<OneTimePrekeyPublic> OneTimePrekeys);

public sealed record ClaimedPrekeyBundle(
    string UserId,
    string DeviceId,
    ProtocolIdentity Identity,
    SignedDevicePrekey SignedPrekey,
    OneTimePrekeyPublic? OneTimePrekey,
    long Sequence,
    DateTimeOffset ExpiresAt);
