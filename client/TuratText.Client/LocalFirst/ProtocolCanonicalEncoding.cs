using System.Text;

namespace TuratText.Client.LocalFirst;

internal static class ProtocolCanonicalEncoding
{
    private static readonly UTF8Encoding StrictUtf8 = new(false, true);

    public static byte[] DeviceCertificate(
        int version,
        string userId,
        string identityAlgorithm,
        string identityPublicKey,
        string deviceId,
        string deviceAlgorithm,
        string devicePublicKey,
        long createdAtUnixMilliseconds)
    {
        using var stream = new MemoryStream();
        using var writer = new BinaryWriter(stream, StrictUtf8, leaveOpen: true);
        writer.Write("TuratText.DeviceCertificate");
        writer.Write(version);
        WriteString(writer, userId);
        WriteString(writer, identityAlgorithm);
        WriteString(writer, identityPublicKey);
        WriteString(writer, deviceId);
        WriteString(writer, deviceAlgorithm);
        WriteString(writer, devicePublicKey);
        writer.Write(createdAtUnixMilliseconds);
        writer.Flush();
        return stream.ToArray();
    }

    public static byte[] Event(SignedProtocolEvent value)
    {
        using var stream = new MemoryStream();
        using var writer = new BinaryWriter(stream, StrictUtf8, leaveOpen: true);
        writer.Write("TuratText.SignedEvent");
        writer.Write(value.Version);
        WriteString(writer, value.EventId);
        WriteString(writer, value.ConversationId);
        WriteString(writer, value.SenderUserId);
        WriteString(writer, value.SenderDeviceId);
        writer.Write(value.DeviceSequence);
        WriteString(writer, value.Kind);
        writer.Write(value.CreatedAtUnixMilliseconds);
        WriteBytes(writer, Convert.FromBase64String(value.Payload));
        writer.Flush();
        return stream.ToArray();
    }

    private static void WriteString(BinaryWriter writer, string value) =>
        WriteBytes(writer, StrictUtf8.GetBytes(value));

    private static void WriteBytes(BinaryWriter writer, byte[] value)
    {
        writer.Write(value.Length);
        writer.Write(value);
    }
}
