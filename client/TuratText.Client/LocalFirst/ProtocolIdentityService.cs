using System.Security.Cryptography;
using System.Text.Json;
using TuratText.Client.Services;

namespace TuratText.Client.LocalFirst;

public sealed class ProtocolIdentityService : IDisposable
{
    private const int ProtocolVersion = 2;
    private const string Algorithm = "ECDSA-P256-SHA256";
    private const string StoragePurpose = "TuratText.ProtocolIdentity.v2";
    private const string IdentityFileName = "protocol-identity-v2.secure";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private ECDsa? _identityKey;
    private ECDsa? _deviceKey;

    public ProtocolIdentityService(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public ProtocolIdentity? Current { get; private set; }
    public bool HasIdentityAuthority => _identityKey is not null;

    public async Task<ProtocolIdentity> GetOrCreateAsync(CancellationToken cancellationToken = default)
    {
        if (Current is not null)
        {
            return Current;
        }

        await _gate.WaitAsync(cancellationToken);
        try
        {
            if (Current is not null)
            {
                return Current;
            }

            string path = IdentityPath;
            if (File.Exists(path))
            {
                byte[] protectedBytes = await File.ReadAllBytesAsync(path, cancellationToken);
                byte[] serialized = _storage.Unprotect(protectedBytes, StoragePurpose);
                StoredProtocolIdentity stored = JsonSerializer.Deserialize<StoredProtocolIdentity>(serialized, JsonOptions)
                                                ?? throw new CryptographicException("Protocol identity file is damaged");
                LoadKeys(stored.IdentityPrivateKey, stored.DevicePrivateKey);
                Current = BuildAndValidateIdentity(stored);
                return Current;
            }

            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            _identityKey = ECDsa.Create(ECCurve.NamedCurves.nistP256);
            _deviceKey = ECDsa.Create(ECCurve.NamedCurves.nistP256);
            long createdAt = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
            string identityPublicKey = Convert.ToBase64String(_identityKey.ExportSubjectPublicKeyInfo());
            string devicePublicKey = Convert.ToBase64String(_deviceKey.ExportSubjectPublicKeyInfo());
            string userId = IdFromPublicKey("tt1", identityPublicKey);
            string deviceId = IdFromPublicKey("ttd1", devicePublicKey);
            byte[] certificateBytes = ProtocolCanonicalEncoding.DeviceCertificate(
                ProtocolVersion,
                userId,
                Algorithm,
                identityPublicKey,
                deviceId,
                Algorithm,
                devicePublicKey,
                createdAt);
            string certificate = Convert.ToBase64String(_identityKey.SignData(
                certificateBytes,
                HashAlgorithmName.SHA256,
                DSASignatureFormat.IeeeP1363FixedFieldConcatenation));

            var storedIdentity = new StoredProtocolIdentity(
                ProtocolVersion,
                Convert.ToBase64String(_identityKey.ExportPkcs8PrivateKey()),
                Convert.ToBase64String(_deviceKey.ExportPkcs8PrivateKey()),
                createdAt,
                certificate,
                identityPublicKey);
            byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(storedIdentity, JsonOptions);
            byte[] encrypted = _storage.Protect(plaintext, StoragePurpose);
            string temporary = path + ".new";
            await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
            File.Move(temporary, path, overwrite: true);

            Current = BuildAndValidateIdentity(storedIdentity);
            return Current;
        }
        finally
        {
            _gate.Release();
        }
    }

    public SignedProtocolEvent SignEvent(
        string conversationId,
        string kind,
        ReadOnlySpan<byte> opaquePayload,
        long deviceSequence,
        DateTimeOffset? createdAt = null)
    {
        ProtocolIdentity identity = Current
                                    ?? throw new InvalidOperationException("Protocol identity is not loaded");
        if (string.IsNullOrWhiteSpace(conversationId))
        {
            throw new ArgumentException("Conversation ID is required", nameof(conversationId));
        }
        if (string.IsNullOrWhiteSpace(kind))
        {
            throw new ArgumentException("Event kind is required", nameof(kind));
        }
        if (deviceSequence <= 0)
        {
            throw new ArgumentOutOfRangeException(nameof(deviceSequence));
        }

        var unsigned = new SignedProtocolEvent(
            ProtocolVersion,
            "evt1-" + Convert.ToHexString(RandomNumberGenerator.GetBytes(16)).ToLowerInvariant(),
            conversationId,
            identity.UserId,
            identity.DeviceId,
            deviceSequence,
            kind,
            (createdAt ?? DateTimeOffset.UtcNow).ToUnixTimeMilliseconds(),
            Convert.ToBase64String(opaquePayload),
            "");
        byte[] signature = _deviceKey!.SignData(
            ProtocolCanonicalEncoding.Event(unsigned),
            HashAlgorithmName.SHA256,
            DSASignatureFormat.IeeeP1363FixedFieldConcatenation);
        return unsigned with { Signature = Convert.ToBase64String(signature) };
    }

    public string SignDeviceData(ReadOnlySpan<byte> value)
    {
        if (Current is null || _deviceKey is null)
        {
            throw new InvalidOperationException("Protocol identity is not loaded");
        }
        return Convert.ToBase64String(_deviceKey.SignData(
            value,
            HashAlgorithmName.SHA256,
            DSASignatureFormat.IeeeP1363FixedFieldConcatenation));
    }

    public string SignIdentityData(ReadOnlySpan<byte> value)
    {
        if (Current is null || _identityKey is null)
        {
            throw new InvalidOperationException("Protocol identity is not loaded");
        }
        return Convert.ToBase64String(_identityKey.SignData(
            value,
            HashAlgorithmName.SHA256,
            DSASignatureFormat.IeeeP1363FixedFieldConcatenation));
    }

    internal PreparedLinkedIdentity PrepareLinkedIdentity()
    {
        if (Current is null || _identityKey is null)
            throw new InvalidOperationException("Only the identity-authority device can link another device");
        using var linkedDeviceKey = ECDsa.Create(ECCurve.NamedCurves.nistP256);
        long createdAt = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        string devicePrivateKey = Convert.ToBase64String(linkedDeviceKey.ExportPkcs8PrivateKey());
        string devicePublicKey = Convert.ToBase64String(linkedDeviceKey.ExportSubjectPublicKeyInfo());
        string deviceId = IdFromPublicKey("ttd1", devicePublicKey);
        string certificate = Convert.ToBase64String(_identityKey.SignData(
            ProtocolCanonicalEncoding.DeviceCertificate(
                ProtocolVersion,
                Current.UserId,
                Algorithm,
                Current.IdentityPublicKey,
                deviceId,
                Algorithm,
                devicePublicKey,
                createdAt),
            HashAlgorithmName.SHA256,
            DSASignatureFormat.IeeeP1363FixedFieldConcatenation));
        var linkedIdentity = new ProtocolIdentity(
            ProtocolVersion,
            Current.UserId,
            Algorithm,
            Current.IdentityPublicKey,
            deviceId,
            Algorithm,
            devicePublicKey,
            createdAt,
            certificate);
        return new PreparedLinkedIdentity(new IdentityLinkMaterial(
            2,
            Current.IdentityPublicKey,
            devicePrivateKey,
            createdAt,
            certificate,
            Current.UserId), linkedIdentity);
    }

    internal async Task<ProtocolIdentity> ImportLinkedIdentityAsync(
        IdentityLinkMaterial material,
        bool replaceExisting,
        CancellationToken cancellationToken = default)
    {
        if (material.Version != 2) throw new CryptographicException("Unsupported device link version");
        await _gate.WaitAsync(cancellationToken);
        try
        {
            if ((Current is not null || File.Exists(IdentityPath)) && !replaceExisting)
                throw new InvalidOperationException("This installation already has an identity");
            _identityKey?.Dispose();
            _deviceKey?.Dispose();
            _identityKey = null;
            string identityPublicKey = material.IdentityPublicKey;
            if (IdFromPublicKey("tt1", identityPublicKey) != material.UserId)
                throw new CryptographicException("Linked identity does not match its UserID");
            _deviceKey = ECDsa.Create();
            _deviceKey.ImportPkcs8PrivateKey(Convert.FromBase64String(material.DevicePrivateKey), out _);
            string devicePublicKey = Convert.ToBase64String(_deviceKey.ExportSubjectPublicKeyInfo());
            var stored = new StoredProtocolIdentity(
                ProtocolVersion,
                "",
                material.DevicePrivateKey,
                material.CreatedAtUnixMilliseconds,
                material.DeviceCertificate,
                identityPublicKey);
            Directory.CreateDirectory(Path.GetDirectoryName(IdentityPath)!);
            string temporary = IdentityPath + ".new";
            await File.WriteAllBytesAsync(
                temporary,
                _storage.Protect(JsonSerializer.SerializeToUtf8Bytes(stored, JsonOptions), StoragePurpose),
                cancellationToken);
            File.Move(temporary, IdentityPath, overwrite: true);
            Current = BuildAndValidateIdentity(stored);
            return Current;
        }
        finally
        {
            _gate.Release();
        }
    }

    public static bool VerifyDeviceData(ProtocolIdentity identity, ReadOnlySpan<byte> value, string signature)
    {
        try
        {
            if (!VerifyDeviceCertificate(identity)) return false;
            using var key = ECDsa.Create();
            key.ImportSubjectPublicKeyInfo(Convert.FromBase64String(identity.DevicePublicKey), out _);
            return key.VerifyData(
                value,
                Convert.FromBase64String(signature),
                HashAlgorithmName.SHA256,
                DSASignatureFormat.IeeeP1363FixedFieldConcatenation);
        }
        catch (Exception exception) when (exception is CryptographicException or FormatException or ArgumentException)
        {
            return false;
        }
    }

    public static bool VerifyIdentityData(ProtocolIdentity identity, ReadOnlySpan<byte> value, string signature)
    {
        try
        {
            if (!VerifyDeviceCertificate(identity)) return false;
            using var key = ECDsa.Create();
            key.ImportSubjectPublicKeyInfo(Convert.FromBase64String(identity.IdentityPublicKey), out _);
            return key.VerifyData(
                value,
                Convert.FromBase64String(signature),
                HashAlgorithmName.SHA256,
                DSASignatureFormat.IeeeP1363FixedFieldConcatenation);
        }
        catch (Exception exception) when (exception is CryptographicException or FormatException or ArgumentException)
        {
            return false;
        }
    }

    public static bool VerifyDeviceCertificate(ProtocolIdentity identity)
    {
        try
        {
            if (identity.Version != ProtocolVersion
                || !string.Equals(identity.IdentityAlgorithm, Algorithm, StringComparison.Ordinal)
                || !string.Equals(identity.DeviceAlgorithm, Algorithm, StringComparison.Ordinal)
                || !string.Equals(identity.UserId, IdFromPublicKey("tt1", identity.IdentityPublicKey), StringComparison.Ordinal)
                || !string.Equals(identity.DeviceId, IdFromPublicKey("ttd1", identity.DevicePublicKey), StringComparison.Ordinal))
            {
                return false;
            }

            using var key = ECDsa.Create();
            key.ImportSubjectPublicKeyInfo(Convert.FromBase64String(identity.IdentityPublicKey), out _);
            return key.VerifyData(
                ProtocolCanonicalEncoding.DeviceCertificate(
                    identity.Version,
                    identity.UserId,
                    identity.IdentityAlgorithm,
                    identity.IdentityPublicKey,
                    identity.DeviceId,
                    identity.DeviceAlgorithm,
                    identity.DevicePublicKey,
                    identity.CreatedAtUnixMilliseconds),
                Convert.FromBase64String(identity.DeviceCertificate),
                HashAlgorithmName.SHA256,
                DSASignatureFormat.IeeeP1363FixedFieldConcatenation);
        }
        catch (Exception exception) when (exception is CryptographicException or FormatException or ArgumentException)
        {
            return false;
        }
    }

    public static bool VerifyEvent(SignedProtocolEvent value, ProtocolIdentity sender)
    {
        try
        {
            if (value.Version != ProtocolVersion
                || value.DeviceSequence <= 0
                || !string.Equals(value.SenderUserId, sender.UserId, StringComparison.Ordinal)
                || !string.Equals(value.SenderDeviceId, sender.DeviceId, StringComparison.Ordinal)
                || !VerifyDeviceCertificate(sender))
            {
                return false;
            }

            using var key = ECDsa.Create();
            key.ImportSubjectPublicKeyInfo(Convert.FromBase64String(sender.DevicePublicKey), out _);
            return key.VerifyData(
                ProtocolCanonicalEncoding.Event(value),
                Convert.FromBase64String(value.Signature),
                HashAlgorithmName.SHA256,
                DSASignatureFormat.IeeeP1363FixedFieldConcatenation);
        }
        catch (Exception exception) when (exception is CryptographicException or FormatException or ArgumentException)
        {
            return false;
        }
    }

    public void Dispose()
    {
        _identityKey?.Dispose();
        _deviceKey?.Dispose();
        _gate.Dispose();
    }

    private ProtocolIdentity BuildAndValidateIdentity(StoredProtocolIdentity stored)
    {
        string identityPublicKey = !string.IsNullOrWhiteSpace(stored.IdentityPublicKey)
            ? stored.IdentityPublicKey
            : Convert.ToBase64String(_identityKey!.ExportSubjectPublicKeyInfo());
        string devicePublicKey = Convert.ToBase64String(_deviceKey!.ExportSubjectPublicKeyInfo());
        var identity = new ProtocolIdentity(
            stored.Version,
            IdFromPublicKey("tt1", identityPublicKey),
            Algorithm,
            identityPublicKey,
            IdFromPublicKey("ttd1", devicePublicKey),
            Algorithm,
            devicePublicKey,
            stored.CreatedAtUnixMilliseconds,
            stored.DeviceCertificate);
        if (!VerifyDeviceCertificate(identity))
        {
            throw new CryptographicException("Device certificate is invalid");
        }
        return identity;
    }

    private void LoadKeys(string identityPrivateKey, string devicePrivateKey)
    {
        if (!string.IsNullOrWhiteSpace(identityPrivateKey))
        {
            _identityKey = ECDsa.Create();
            _identityKey.ImportPkcs8PrivateKey(Convert.FromBase64String(identityPrivateKey), out _);
        }
        else
        {
            _identityKey = null;
        }
        _deviceKey = ECDsa.Create();
        _deviceKey.ImportPkcs8PrivateKey(Convert.FromBase64String(devicePrivateKey), out _);
    }

    private string IdentityPath => Path.Combine(_storage.AppDirectory, "local-first", IdentityFileName);

    private static string IdFromPublicKey(string prefix, string publicKey)
    {
        byte[] hash = SHA256.HashData(Convert.FromBase64String(publicKey));
        return prefix + "-" + Convert.ToHexString(hash).ToLowerInvariant();
    }

    private sealed record StoredProtocolIdentity(
        int Version,
        string IdentityPrivateKey,
        string DevicePrivateKey,
        long CreatedAtUnixMilliseconds,
        string DeviceCertificate,
        string? IdentityPublicKey);
}

internal sealed record IdentityLinkMaterial(
    int Version,
    string IdentityPublicKey,
    string DevicePrivateKey,
    long CreatedAtUnixMilliseconds,
    string DeviceCertificate,
    string UserId);

internal sealed record PreparedLinkedIdentity(IdentityLinkMaterial Material, ProtocolIdentity Identity);
