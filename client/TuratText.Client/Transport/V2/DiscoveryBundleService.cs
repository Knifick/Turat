using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.LocalFirst;

namespace TuratText.Client.Transport.V2;

public sealed record DiscoveryBundleBody(
    int Version,
    long CreatedAtUnixMilliseconds,
    long ExpiresAtUnixMilliseconds,
    ProtocolIdentity Sender,
    IReadOnlyList<NodeDescriptor> Nodes,
    IReadOnlyList<RelayDescriptor> Relays);

public sealed record SignedDiscoveryBundle(
    DiscoveryBundleBody Body,
    string BodyJson,
    string Signature);

public sealed class DiscoveryBundleService
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web) { WriteIndented = true };
    private static readonly JsonSerializerOptions CanonicalJsonOptions = new(JsonSerializerDefaults.Web);
    private readonly ProtocolIdentityService _identity;
    private readonly OwnedMailboxStore _mailboxes;
    private readonly BootstrapNodeSource _bootstrap;
    private readonly RelayDescriptorSource _relays;

    public DiscoveryBundleService(
        ProtocolIdentityService identity,
        OwnedMailboxStore mailboxes,
        BootstrapNodeSource bootstrap,
        RelayDescriptorSource relays)
    {
        _identity = identity;
        _mailboxes = mailboxes;
        _bootstrap = bootstrap;
        _relays = relays;
    }

    public async Task<byte[]> ExportAsync(CancellationToken cancellationToken = default)
    {
        ProtocolIdentity sender = await _identity.GetOrCreateAsync(cancellationToken);
        IReadOnlyList<NodeDescriptor> nodes = (await _mailboxes.LoadAsync(cancellationToken))
            .Select(value => value.Node)
            .Where(value => NodeDescriptorVerifier.Verify(value))
            .DistinctBy(value => value.NodeId)
            .Take(20)
            .ToList();
        IReadOnlyList<RelayDescriptor> relays = (await _relays.LoadAsync(cancellationToken))
            .Take(50)
            .ToList();
        long now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        var body = new DiscoveryBundleBody(
            2,
            now,
            DateTimeOffset.UtcNow.AddDays(7).ToUnixTimeMilliseconds(),
            sender,
            nodes,
            relays);
        string bodyJson = JsonSerializer.Serialize(body, CanonicalJsonOptions);
        var bundle = new SignedDiscoveryBundle(
            body,
            bodyJson,
            _identity.SignDeviceData(Encoding.UTF8.GetBytes(bodyJson)));
        return JsonSerializer.SerializeToUtf8Bytes(bundle, JsonOptions);
    }

    public async Task<(int Nodes, int Relays)> ImportAsync(
        ReadOnlyMemory<byte> encoded,
        CancellationToken cancellationToken = default)
    {
        if (encoded.Length > 1_000_000) throw new CryptographicException("Discovery bundle is too large");
        SignedDiscoveryBundle bundle = JsonSerializer.Deserialize<SignedDiscoveryBundle>(encoded.Span, JsonOptions)
                                         ?? throw new CryptographicException("Discovery bundle is empty");
        if (!Verify(bundle)) throw new CryptographicException("Discovery bundle signature or descriptors are invalid");
        foreach (NodeDescriptor node in bundle.Body.Nodes) await _bootstrap.AddPinnedAsync(node, cancellationToken);
        foreach (RelayDescriptor relay in bundle.Body.Relays) await _relays.AddPinnedAsync(relay, cancellationToken);
        return (bundle.Body.Nodes.Count, bundle.Body.Relays.Count);
    }

    public static bool Verify(SignedDiscoveryBundle bundle)
    {
        try
        {
            long now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
            return bundle.Body.Version == 2
                   && bundle.Body.CreatedAtUnixMilliseconds <= now + TimeSpan.FromMinutes(10).TotalMilliseconds
                   && bundle.Body.ExpiresAtUnixMilliseconds > now
                   && bundle.Body.ExpiresAtUnixMilliseconds
                   <= bundle.Body.CreatedAtUnixMilliseconds + TimeSpan.FromDays(8).TotalMilliseconds
                   && bundle.Body.Nodes.Count <= 20
                   && bundle.Body.Relays.Count <= 50
                   && bundle.Body.Nodes.All(value => NodeDescriptorVerifier.Verify(value))
                   && bundle.Body.Relays.All(RelayDescriptorVerifier.Verify)
                   && ProtocolIdentityService.VerifyDeviceCertificate(bundle.Body.Sender)
                   && bundle.BodyJson == JsonSerializer.Serialize(bundle.Body, CanonicalJsonOptions)
                   && ProtocolIdentityService.VerifyDeviceData(
                       bundle.Body.Sender,
                       Encoding.UTF8.GetBytes(bundle.BodyJson),
                       bundle.Signature);
        }
        catch
        {
            return false;
        }
    }
}
