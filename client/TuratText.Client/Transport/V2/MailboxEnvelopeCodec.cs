using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;

namespace TuratText.Client.Transport.V2;

public static class MailboxEnvelopeCodec
{
    private const byte FormatVersion = 1;
    private const int NonceSize = 12;
    private const int TagSize = 16;
    private static readonly int[] SizeClasses = [1024, 4096, 16384, 65536, 262144, 524288];

    public static MailboxEnvelope Encode(
        PublicMailboxRoute route,
        ReadOnlySpan<byte> innerE2eeMessage,
        TimeSpan ttl)
    {
        int minimumPackedLength = checked(1 + NonceSize + 4 + innerE2eeMessage.Length + TagSize);
        int packedSizeClass = SizeClasses.FirstOrDefault(value => value >= minimumPackedLength);
        if (packedSizeClass == 0) throw new ArgumentOutOfRangeException(nameof(innerE2eeMessage));
        int paddedLength = packedSizeClass - 1 - NonceSize - TagSize;
        byte[] plaintext = RandomNumberGenerator.GetBytes(paddedLength);
        BinaryPrimitives.WriteInt32BigEndian(plaintext, innerE2eeMessage.Length);
        innerE2eeMessage.CopyTo(plaintext.AsSpan(4));

        byte[] nonce = RandomNumberGenerator.GetBytes(NonceSize);
        byte[] ciphertext = new byte[plaintext.Length];
        byte[] tag = new byte[TagSize];
        byte[] key = DeriveOuterKey(route.MailboxId, route.WriteCapability);
        using (var aes = new AesGcm(key, TagSize))
        {
            aes.Encrypt(nonce, plaintext, ciphertext, tag, Encoding.UTF8.GetBytes(route.NodeId));
        }
        CryptographicOperations.ZeroMemory(key);
        byte[] packed = new byte[1 + NonceSize + ciphertext.Length + TagSize];
        packed[0] = FormatVersion;
        nonce.CopyTo(packed, 1);
        ciphertext.CopyTo(packed, 1 + NonceSize);
        tag.CopyTo(packed, 1 + NonceSize + ciphertext.Length);
        DateTimeOffset now = DateTimeOffset.UtcNow;
        return new MailboxEnvelope(
            "env1-" + RandomToken(18),
            2,
            null,
            Convert.ToBase64String(packed),
            packedSizeClass,
            now,
            now.Add(ttl));
    }

    public static byte[] Decode(OwnedMailboxRoute route, MailboxEnvelope envelope)
    {
        byte[] packed = Convert.FromBase64String(envelope.OpaquePayload);
        if (packed.Length < 1 + NonceSize + TagSize + 4 || packed[0] != FormatVersion)
        {
            throw new CryptographicException("Unsupported mailbox envelope format");
        }
        ReadOnlySpan<byte> nonce = packed.AsSpan(1, NonceSize);
        ReadOnlySpan<byte> ciphertext = packed.AsSpan(1 + NonceSize, packed.Length - 1 - NonceSize - TagSize);
        ReadOnlySpan<byte> tag = packed.AsSpan(packed.Length - TagSize);
        byte[] plaintext = TryDecrypt(
            route.MailboxId,
            route.Node.NodeId,
            route.WriteCapability,
            nonce,
            ciphertext,
            tag);
        if (plaintext.Length == 0)
        {
            plaintext = TryDecrypt(
                route.MailboxId,
                route.Node.NodeId,
                route.ContactCapability,
                nonce,
                ciphertext,
                tag,
                throwOnFailure: true);
        }
        int length = BinaryPrimitives.ReadInt32BigEndian(plaintext);
        if (length < 0 || length > plaintext.Length - 4)
        {
            throw new CryptographicException("Mailbox envelope length is invalid");
        }
        return plaintext.AsSpan(4, length).ToArray();
    }

    private static byte[] TryDecrypt(
        Guid mailboxId,
        string nodeId,
        string capability,
        ReadOnlySpan<byte> nonce,
        ReadOnlySpan<byte> ciphertext,
        ReadOnlySpan<byte> tag,
        bool throwOnFailure = false)
    {
        byte[] plaintext = new byte[ciphertext.Length];
        byte[] key = DeriveOuterKey(mailboxId, capability);
        try
        {
            using var aes = new AesGcm(key, TagSize);
            aes.Decrypt(nonce, ciphertext, tag, plaintext, Encoding.UTF8.GetBytes(nodeId));
            return plaintext;
        }
        catch (CryptographicException) when (!throwOnFailure)
        {
            CryptographicOperations.ZeroMemory(plaintext);
            return [];
        }
        finally
        {
            CryptographicOperations.ZeroMemory(key);
        }
    }

    private static byte[] DeriveOuterKey(Guid mailboxId, string writeCapability)
    {
        byte[] material = Encoding.UTF8.GetBytes(
            "TuratText.MailboxOuter.v1\0" + mailboxId.ToString("N") + "\0" + writeCapability);
        return SHA256.HashData(material);
    }

    private static string RandomToken(int bytes) =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
            .TrimEnd('=').Replace('+', '-').Replace('/', '_');
}
