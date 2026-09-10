namespace TuratText.Client.LocalFirst;

public sealed record SignedProtocolEvent(
    int Version,
    string EventId,
    string ConversationId,
    string SenderUserId,
    string SenderDeviceId,
    long DeviceSequence,
    string Kind,
    long CreatedAtUnixMilliseconds,
    string Payload,
    string Signature);

public enum EventDeliveryState
{
    LocalOnly,
    Pending,
    Delivered,
    Incoming
}
