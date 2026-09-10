using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;
using Org.BouncyCastle.Crypto.Parameters;
using Org.BouncyCastle.Crypto.Signers;
using Org.BouncyCastle.Security;

namespace TuratText.Client.Transport.V2;

public static class NodeDescriptorVerifier
{
    public static bool Verify(NodeDescriptor descriptor, string? expectedNodeId = null)
    {
        try
        {
            if (descriptor.Version != 2
                || !string.Equals(descriptor.Algorithm, "Ed25519", StringComparison.Ordinal)
                || descriptor.ExpiresAtUnixMilliseconds <= DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
                || (expectedNodeId is not null
                    && !string.Equals(expectedNodeId, descriptor.NodeId, StringComparison.Ordinal)))
            {
                return false;
            }

            byte[] spki = Convert.FromBase64String(descriptor.PublicKey);
            string calculatedId = "ttn1-" + Convert.ToHexString(SHA256.HashData(spki)).ToLowerInvariant();
            if (!string.Equals(calculatedId, descriptor.NodeId, StringComparison.Ordinal))
            {
                return false;
            }

            var publicKey = (Ed25519PublicKeyParameters)PublicKeyFactory.CreateKey(spki);
            var verifier = new Ed25519Signer();
            verifier.Init(false, publicKey);
            byte[] canonical = Canonical(descriptor);
            verifier.BlockUpdate(canonical, 0, canonical.Length);
            return verifier.VerifySignature(Convert.FromBase64String(descriptor.Signature));
        }
        catch (Exception exception) when (exception is ArgumentException or InvalidCastException or IOException)
        {
            return false;
        }
    }

    private static byte[] Canonical(NodeDescriptor value)
    {
        using var stream = new MemoryStream();
        WriteString(stream, "TuratText.NodeDescriptor");
        WriteInt32(stream, value.Version);
        WriteString(stream, value.NodeId);
        WriteString(stream, value.Name);
        WriteString(stream, value.BaseUrl);
        WriteString(stream, value.PublicKey);
        WriteInt64(stream, value.ExpiresAtUnixMilliseconds);
        WriteInt32(stream, value.Transports.Count);
        foreach (string transport in value.Transports)
        {
            WriteString(stream, transport);
        }
        WriteInt32(stream, value.RegistrationPowBits);
        WriteInt32(stream, value.EnvelopePowBits);
        WriteInt32(stream, value.ContactPowBits);
        WriteInt32(stream, value.MaxEnvelopeBytes);
        WriteInt64(stream, value.MaxMailboxTtlHours);
        return stream.ToArray();
    }

    private static void WriteString(Stream stream, string value)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(value);
        WriteInt32(stream, bytes.Length);
        stream.Write(bytes);
    }

    private static void WriteInt32(Stream stream, int value)
    {
        Span<byte> bytes = stackalloc byte[4];
        BinaryPrimitives.WriteInt32BigEndian(bytes, value);
        stream.Write(bytes);
    }

    private static void WriteInt64(Stream stream, long value)
    {
        Span<byte> bytes = stackalloc byte[8];
        BinaryPrimitives.WriteInt64BigEndian(bytes, value);
        stream.Write(bytes);
    }
}
