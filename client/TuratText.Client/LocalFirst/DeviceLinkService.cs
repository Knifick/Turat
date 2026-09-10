using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.Services;

namespace TuratText.Client.LocalFirst;

public sealed record DeviceLinkResult(ProtocolIdentity Identity, SignedDeviceList DeviceList);

public sealed class DeviceLinkService
{
    private static readonly byte[] Magic = "TTLINKV2"u8.ToArray();
    private const int Iterations = 600_000;
    private const int TagSize = 16;
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly DeviceListService _devices;

    public DeviceLinkService(
        IProtectedStorage storage,
        ProtocolIdentityService identity,
        DeviceListService devices)
    {
        _storage = storage;
        _identity = identity;
        _devices = devices;
    }

    public async Task<byte[]> CreatePackageAsync(
        string passphrase,
        TimeSpan? lifetime = null,
        CancellationToken cancellationToken = default)
    {
        ValidatePassphrase(passphrase);
        await _identity.GetOrCreateAsync(cancellationToken);
        PreparedLinkedIdentity linked = _identity.PrepareLinkedIdentity();
        SignedDeviceList devices = await _devices.AddLinkedDeviceAsync(linked.Identity, cancellationToken);
        var payload = new LinkPayload(
            2,
            linked.Material,
            devices,
            DateTimeOffset.UtcNow.Add(lifetime ?? TimeSpan.FromMinutes(15)).ToUnixTimeMilliseconds(),
            Convert.ToBase64String(RandomNumberGenerator.GetBytes(16)));
        return Encrypt(JsonSerializer.SerializeToUtf8Bytes(payload, JsonOptions), passphrase);
    }

    public async Task<DeviceLinkResult> ImportPackageAsync(
        ReadOnlyMemory<byte> package,
        string passphrase,
        bool replaceExistingInstallation,
        CancellationToken cancellationToken = default)
    {
        ValidatePassphrase(passphrase);
        LinkPayload payload = JsonSerializer.Deserialize<LinkPayload>(Decrypt(package.Span, passphrase), JsonOptions)
                              ?? throw new CryptographicException("Device link package is invalid");
        if (payload.Version != 2 || payload.ExpiresAtUnixMilliseconds < DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
            || !DeviceListService.Verify(payload.DeviceList)
            || payload.DeviceList.Document.UserId != payload.Identity.UserId)
            throw new CryptographicException("Device link package is expired or invalid");
        if (replaceExistingInstallation) ResetDeviceSpecificState();
        ProtocolIdentity identity = await _identity.ImportLinkedIdentityAsync(
            payload.Identity,
            replaceExistingInstallation,
            cancellationToken);
        await _devices.ApplyAsync(payload.DeviceList, cancellationToken);
        SignedDeviceList updated = await _devices.GetOrCreateAsync(cancellationToken);
        return new DeviceLinkResult(identity, updated);
    }

    public Task<SignedDeviceList> RevokeDeviceAsync(
        string deviceId,
        string reason,
        CancellationToken cancellationToken = default) =>
        _devices.RevokeAsync(deviceId.Trim(), reason, cancellationToken);

    private void ResetDeviceSpecificState()
    {
        string local = Path.Combine(_storage.AppDirectory, "local-first");
        foreach (string name in new[]
                 {
                     "prekeys-v2.secure", "ratchet-sessions-v2.secure", "owned-mailboxes-v2.secure",
                     "routing-sequence-v2.secure", "device-list-v2.secure", "contacts-v2.secure",
                     "own-routing-v2.secure", "private-mailbox-grants-v2.secure", "username-sequence-v2.secure",
                     "update-state-v2.secure",
                     "metadata-protection-v2.secure",
                     "group-epochs-v2.secure", "transparency-v2.secure", "events-v2.db",
                     "events-v2.db-wal", "events-v2.db-shm", "event-vault-key-v1.secure"
                 })
        {
            string path = Path.GetFullPath(Path.Combine(local, name));
            if (!path.StartsWith(Path.GetFullPath(local) + Path.DirectorySeparatorChar, StringComparison.OrdinalIgnoreCase))
                throw new InvalidOperationException("Unsafe device state path");
            if (File.Exists(path)) File.Delete(path);
        }
    }

    private static byte[] Encrypt(byte[] plaintext, string passphrase)
    {
        byte[] salt = RandomNumberGenerator.GetBytes(16);
        byte[] nonce = RandomNumberGenerator.GetBytes(12);
        byte[] key = Rfc2898DeriveBytes.Pbkdf2(passphrase, salt, Iterations, HashAlgorithmName.SHA256, 32);
        byte[] ciphertext = new byte[plaintext.Length];
        byte[] tag = new byte[TagSize];
        using (var aes = new AesGcm(key, TagSize)) aes.Encrypt(nonce, plaintext, ciphertext, tag, Magic);
        CryptographicOperations.ZeroMemory(key);
        return [.. Magic, .. salt, .. nonce, .. tag, .. ciphertext];
    }

    private static byte[] Decrypt(ReadOnlySpan<byte> value, string passphrase)
    {
        int header = Magic.Length + 16 + 12 + TagSize;
        if (value.Length <= header || !value[..Magic.Length].SequenceEqual(Magic))
            throw new CryptographicException("Device link package format is invalid");
        byte[] salt = value.Slice(Magic.Length, 16).ToArray();
        byte[] nonce = value.Slice(Magic.Length + 16, 12).ToArray();
        byte[] tag = value.Slice(Magic.Length + 28, TagSize).ToArray();
        byte[] ciphertext = value[header..].ToArray();
        byte[] key = Rfc2898DeriveBytes.Pbkdf2(passphrase, salt, Iterations, HashAlgorithmName.SHA256, 32);
        byte[] plaintext = new byte[ciphertext.Length];
        using (var aes = new AesGcm(key, TagSize)) aes.Decrypt(nonce, ciphertext, tag, plaintext, Magic);
        CryptographicOperations.ZeroMemory(key);
        return plaintext;
    }

    private static void ValidatePassphrase(string value)
    {
        if (string.IsNullOrWhiteSpace(value) || value.Length < 10)
            throw new ArgumentException("Link passphrase must contain at least 10 characters", nameof(value));
    }

    private sealed record LinkPayload(
        int Version,
        IdentityLinkMaterial Identity,
        SignedDeviceList DeviceList,
        long ExpiresAtUnixMilliseconds,
        string PackageNonce);
}
