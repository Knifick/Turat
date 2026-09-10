namespace TuratText.Client.Transport.V2;

public sealed record NodeDescriptor(
    int Version,
    string NodeId,
    string Name,
    string BaseUrl,
    string PublicKey,
    string Algorithm,
    long ExpiresAtUnixMilliseconds,
    IReadOnlyList<string> Transports,
    int RegistrationPowBits,
    int EnvelopePowBits,
    int ContactPowBits,
    int MaxEnvelopeBytes,
    long MaxMailboxTtlHours,
    string Signature);

public sealed record OwnedMailboxRoute(
    NodeDescriptor Node,
    Guid MailboxId,
    string DeviceHint,
    string ReadCapability,
    string WriteCapability,
    string ContactCapability,
    DateTimeOffset CreatedAt,
    DateTimeOffset ExpiresAt);

public sealed record PublicMailboxRoute(
    string NodeId,
    string BaseUrl,
    string NodePublicKey,
    Guid MailboxId,
    string DeviceHint,
    string WriteCapability,
    DateTimeOffset ExpiresAt,
    int EnvelopePowBits,
    IReadOnlyList<string> Transports);

public sealed record MailboxEnvelope(
    string EnvelopeId,
    int ProtocolVersion,
    string? RecipientDeviceHint,
    string OpaquePayload,
    int SizeClass,
    DateTimeOffset CreatedAt,
    DateTimeOffset ExpiresAt);
