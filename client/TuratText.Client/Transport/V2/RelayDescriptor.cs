namespace TuratText.Client.Transport.V2;

public sealed record RelayDescriptor(
    int Version,
    string RelayId,
    string ConnectHost,
    int ConnectPort,
    string TargetHost,
    int TargetPort,
    string PublicKey,
    string Algorithm,
    long ExpiresAtUnixMilliseconds,
    IReadOnlyList<string> Transports,
    string Signature);

