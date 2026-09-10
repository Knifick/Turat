using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;
using Org.BouncyCastle.Crypto.Parameters;
using Org.BouncyCastle.Crypto.Signers;
using Org.BouncyCastle.Security;

namespace TuratText.Client.Transport.V2;

public static class RelayDescriptorVerifier
{
    public static bool Verify(RelayDescriptor value)
    {
        try
        {
            if (value.Version != 2
                || value.Algorithm != "Ed25519"
                || value.ConnectPort is < 1 or > 65535
                || value.TargetPort is < 1 or > 65535
                || string.IsNullOrWhiteSpace(value.ConnectHost)
                || string.IsNullOrWhiteSpace(value.TargetHost)
                || value.ExpiresAtUnixMilliseconds <= DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
                || !value.Transports.Contains("tls-tcp", StringComparer.Ordinal)) return false;
            byte[] spki = Convert.FromBase64String(value.PublicKey);
            string expectedId = "ttr1-" + Convert.ToHexString(SHA256.HashData(spki)).ToLowerInvariant();
            if (expectedId != value.RelayId) return false;
            var key = (Ed25519PublicKeyParameters)PublicKeyFactory.CreateKey(spki);
            var verifier = new Ed25519Signer();
            verifier.Init(false, key);
            byte[] canonical = Canonical(value);
            verifier.BlockUpdate(canonical, 0, canonical.Length);
            return verifier.VerifySignature(Convert.FromBase64String(value.Signature));
        }
        catch
        {
            return false;
        }
    }

    public static byte[] Canonical(RelayDescriptor value)
    {
        using var stream = new MemoryStream();
        WriteString(stream, "TuratText.RelayDescriptor");
        WriteInt32(stream, value.Version);
        WriteString(stream, value.RelayId);
        WriteString(stream, value.ConnectHost);
        WriteInt32(stream, value.ConnectPort);
        WriteString(stream, value.TargetHost);
        WriteInt32(stream, value.TargetPort);
        WriteString(stream, value.PublicKey);
        WriteInt64(stream, value.ExpiresAtUnixMilliseconds);
        WriteInt32(stream, value.Transports.Count);
        foreach (string transport in value.Transports) WriteString(stream, transport);
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

