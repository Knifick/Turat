using System.Security.Cryptography;
using System.Text.Json;
using TuratText.Client.LocalFirst;
using TuratText.Client.Services;
using TuratText.Client.Transport.V2;

namespace TuratText.Client.Messaging.V2;

public sealed record PrivateMailboxGrant(
    NodeDescriptor Node,
    Guid MailboxId,
    string DeviceHint,
    string WriteCapability,
    DateTimeOffset ExpiresAt);

public sealed class PrivateMailboxGrantStore
{
    private const string Purpose = "TuratText.PrivateMailboxGrants.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public PrivateMailboxGrantStore(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public async Task ApplyAsync(
        SignedRoutingDescriptor routing,
        ProtocolIdentity sender,
        IReadOnlyCollection<PrivateMailboxGrant> grants,
        CancellationToken cancellationToken = default)
    {
        if (!RoutingDescriptorService.Verify(routing)
            || routing.Descriptor.UserId != sender.UserId
            || grants.Count > 20
            || grants.Any(grant => !VerifyGrant(routing, sender, grant)))
        {
            throw new CryptographicException("Private mailbox grant does not match signed routing");
        }
        await _gate.WaitAsync(cancellationToken);
        try
        {
            List<StoredGrant> values = await LoadUnlockedAsync(cancellationToken);
            values.RemoveAll(value => value.UserId == sender.UserId && value.DeviceId == sender.DeviceId);
            values.AddRange(grants.Select(value => new StoredGrant(sender.UserId, sender.DeviceId, value)));
            await SaveUnlockedAsync(values, cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<IReadOnlyList<PublicMailboxRoute>> LoadRoutesAsync(
        string userId,
        string deviceId,
        CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            return (await LoadUnlockedAsync(cancellationToken))
                .Where(value => value.UserId == userId
                                && value.DeviceId == deviceId
                                && value.Grant.ExpiresAt > DateTimeOffset.UtcNow
                                && NodeDescriptorVerifier.Verify(value.Grant.Node))
                .Select(value => new PublicMailboxRoute(
                    value.Grant.Node.NodeId,
                    value.Grant.Node.BaseUrl,
                    value.Grant.Node.PublicKey,
                    value.Grant.MailboxId,
                    value.Grant.DeviceHint,
                    value.Grant.WriteCapability,
                    value.Grant.ExpiresAt,
                    value.Grant.Node.EnvelopePowBits,
                    value.Grant.Node.Transports))
                .ToList();
        }
        finally
        {
            _gate.Release();
        }
    }

    public static bool VerifyGrant(
        SignedRoutingDescriptor routing,
        ProtocolIdentity sender,
        PrivateMailboxGrant grant)
    {
        if (!NodeDescriptorVerifier.Verify(grant.Node)
            || grant.WriteCapability.Length < 32
            || grant.ExpiresAt <= DateTimeOffset.UtcNow) return false;
        DeviceRoutingEntry? device = routing.Descriptor.Devices.FirstOrDefault(
            value => value.Identity.DeviceId == sender.DeviceId);
        return device?.Mailboxes.Any(value =>
            value.NodeId == grant.Node.NodeId
            && value.MailboxId == grant.MailboxId
            && value.DeviceHint == grant.DeviceHint
            && value.ExpiresAt == grant.ExpiresAt
            && value.NodePublicKey == grant.Node.PublicKey) == true;
    }

    private async Task<List<StoredGrant>> LoadUnlockedAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(StorePath)) return [];
        byte[] encrypted = await File.ReadAllBytesAsync(StorePath, cancellationToken);
        return JsonSerializer.Deserialize<List<StoredGrant>>(_storage.Unprotect(encrypted, Purpose), JsonOptions) ?? [];
    }

    private async Task SaveUnlockedAsync(List<StoredGrant> values, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(StorePath)!);
        byte[] encrypted = _storage.Protect(JsonSerializer.SerializeToUtf8Bytes(values, JsonOptions), Purpose);
        string temporary = StorePath + ".new";
        await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
        File.Move(temporary, StorePath, true);
    }

    private string StorePath => Path.Combine(_storage.AppDirectory, "local-first", "private-mailbox-grants-v2.secure");

    private sealed record StoredGrant(string UserId, string DeviceId, PrivateMailboxGrant Grant);
}
