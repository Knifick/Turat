using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.Services;

namespace TuratText.Client.LocalFirst;

public sealed record DeviceRevocation(string DeviceId, long RevokedAtUnixMilliseconds, string Reason);

public sealed record DeviceListDocument(
    int Version,
    string UserId,
    long Sequence,
    long UpdatedAtUnixMilliseconds,
    IReadOnlyList<ProtocolIdentity> Devices,
    IReadOnlyList<DeviceRevocation> Revocations);

public sealed record SignedDeviceList(DeviceListDocument Document, string DocumentJson, string Signature);

public sealed class DeviceListService
{
    private const string StoragePurpose = "TuratText.DeviceList.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public DeviceListService(IProtectedStorage storage, ProtocolIdentityService identity)
    {
        _storage = storage;
        _identity = identity;
    }

    public async Task<SignedDeviceList> GetOrCreateAsync(CancellationToken cancellationToken = default)
    {
        ProtocolIdentity current = await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            SignedDeviceList? stored = await LoadAsync(cancellationToken);
            if (stored is null) return await SaveNewAsync([current], [], 1, cancellationToken);
            if (!Verify(stored)) throw new CryptographicException("Stored DeviceList is invalid");
            if (stored.Document.Devices.Any(value => value.DeviceId == current.DeviceId)) return stored;
            if (stored.Document.Revocations.Any(value => value.DeviceId == current.DeviceId)) return stored;
            List<ProtocolIdentity> devices = stored.Document.Devices.ToList();
            devices.Add(current);
            return await SaveNewAsync(devices, stored.Document.Revocations, checked(stored.Document.Sequence + 1), cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<SignedDeviceList> RevokeAsync(
        string deviceId,
        string reason,
        CancellationToken cancellationToken = default)
    {
        if (!_identity.HasIdentityAuthority)
            throw new InvalidOperationException("Only the identity-authority device can revoke another device");
        await _gate.WaitAsync(cancellationToken);
        try
        {
            SignedDeviceList current = await LoadAsync(cancellationToken)
                                       ?? throw new InvalidOperationException("DeviceList was not initialized");
            if (!current.Document.Devices.Any(value => value.DeviceId == deviceId))
                throw new InvalidOperationException("Device is not active");
            List<ProtocolIdentity> devices = current.Document.Devices.Where(value => value.DeviceId != deviceId).ToList();
            if (devices.Count == 0) throw new InvalidOperationException("The last active device cannot be revoked");
            List<DeviceRevocation> revocations = current.Document.Revocations
                .Where(value => value.DeviceId != deviceId).ToList();
            revocations.Add(new DeviceRevocation(deviceId, DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(), reason.Trim()));
            return await SaveNewAsync(devices, revocations, checked(current.Document.Sequence + 1), cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    internal async Task<SignedDeviceList> AddLinkedDeviceAsync(
        ProtocolIdentity linkedDevice,
        CancellationToken cancellationToken = default)
    {
        if (!_identity.HasIdentityAuthority)
            throw new InvalidOperationException("Only the identity-authority device can link another device");
        if (!ProtocolIdentityService.VerifyDeviceCertificate(linkedDevice)
            || linkedDevice.UserId != _identity.Current?.UserId)
            throw new CryptographicException("Linked device certificate is invalid");
        await _gate.WaitAsync(cancellationToken);
        try
        {
            SignedDeviceList current = await LoadAsync(cancellationToken)
                                       ?? await SaveNewAsync([_identity.Current!], [], 1, cancellationToken);
            if (current.Document.Revocations.Any(value => value.DeviceId == linkedDevice.DeviceId))
                throw new CryptographicException("A revoked DeviceID cannot be linked again");
            if (current.Document.Devices.Any(value => value.DeviceId == linkedDevice.DeviceId)) return current;
            return await SaveNewAsync(
                current.Document.Devices.Append(linkedDevice).ToList(),
                current.Document.Revocations,
                checked(current.Document.Sequence + 1),
                cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task ApplyAsync(SignedDeviceList incoming, CancellationToken cancellationToken = default)
    {
        if (!Verify(incoming)) throw new CryptographicException("Incoming DeviceList is invalid");
        await _gate.WaitAsync(cancellationToken);
        try
        {
            SignedDeviceList? current = await LoadAsync(cancellationToken);
            if (current is not null)
            {
                if (incoming.Document.UserId != current.Document.UserId
                    || incoming.Document.Sequence < current.Document.Sequence)
                    throw new CryptographicException("DeviceList rollback detected");
                if (incoming.Document.Sequence == current.Document.Sequence)
                {
                    if (incoming.Signature != current.Signature)
                        throw new CryptographicException("Conflicting DeviceList fork detected");
                    return;
                }
            }
            await WriteAsync(incoming, cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    public static bool Verify(SignedDeviceList value)
    {
        try
        {
            if (value.Document.Version != 2 || value.Document.Sequence <= 0 || value.Document.Devices.Count == 0
                || value.Document.Devices.Any(device => device.UserId != value.Document.UserId
                                                        || !ProtocolIdentityService.VerifyDeviceCertificate(device))
                || value.Document.Devices.Select(device => device.DeviceId).Distinct().Count() != value.Document.Devices.Count
                || value.Document.Revocations.Any(revocation =>
                    value.Document.Devices.Any(device => device.DeviceId == revocation.DeviceId))) return false;
            string json = JsonSerializer.Serialize(value.Document, JsonOptions);
            if (json != value.DocumentJson) return false;
            return ProtocolIdentityService.VerifyIdentityData(
                value.Document.Devices[0], Encoding.UTF8.GetBytes(json), value.Signature);
        }
        catch
        {
            return false;
        }
    }

    private async Task<SignedDeviceList> SaveNewAsync(
        IReadOnlyCollection<ProtocolIdentity> devices,
        IReadOnlyCollection<DeviceRevocation> revocations,
        long sequence,
        CancellationToken cancellationToken)
    {
        var document = new DeviceListDocument(
            2,
            _identity.Current!.UserId,
            sequence,
            DateTimeOffset.UtcNow.ToUnixTimeMilliseconds(),
            devices.OrderBy(value => value.DeviceId, StringComparer.Ordinal).ToList(),
            revocations.OrderBy(value => value.DeviceId, StringComparer.Ordinal).ToList());
        string json = JsonSerializer.Serialize(document, JsonOptions);
        var signed = new SignedDeviceList(document, json, _identity.SignIdentityData(Encoding.UTF8.GetBytes(json)));
        await WriteAsync(signed, cancellationToken);
        return signed;
    }

    private async Task<SignedDeviceList?> LoadAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(Path)) return null;
        return JsonSerializer.Deserialize<SignedDeviceList>(
            _storage.Unprotect(await File.ReadAllBytesAsync(Path, cancellationToken), StoragePurpose), JsonOptions);
    }

    private async Task WriteAsync(SignedDeviceList value, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
        string temporary = Path + ".new";
        await File.WriteAllBytesAsync(temporary, _storage.Protect(
            JsonSerializer.SerializeToUtf8Bytes(value, JsonOptions), StoragePurpose), cancellationToken);
        File.Move(temporary, Path, overwrite: true);
    }

    private string Path => System.IO.Path.Combine(_storage.AppDirectory, "local-first", "device-list-v2.secure");
}
