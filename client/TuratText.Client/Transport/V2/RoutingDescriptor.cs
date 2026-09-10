using TuratText.Client.LocalFirst;

namespace TuratText.Client.Transport.V2;

public sealed record DeviceRoutingEntry(
    ProtocolIdentity Identity,
    IReadOnlyList<PublicMailboxRoute> Mailboxes,
    long PrekeySequence);

public sealed record RoutingDescriptor(
    int Version,
    string UserId,
    long Sequence,
    long CreatedAtUnixMilliseconds,
    long ExpiresAtUnixMilliseconds,
    SignedDeviceList DeviceList,
    IReadOnlyList<DeviceRoutingEntry> Devices);

public sealed record SignedRoutingDescriptor(
    RoutingDescriptor Descriptor,
    string DescriptorJson,
    string Signature);
