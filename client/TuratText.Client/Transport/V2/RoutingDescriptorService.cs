using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.Crypto.V2;
using TuratText.Client.LocalFirst;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed class RoutingDescriptorService
{
    private const string SequencePurpose = "TuratText.RoutingSequence.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly PrekeyStateService _prekeys;
    private readonly DeviceListService _devices;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public RoutingDescriptorService(
        IProtectedStorage storage,
        ProtocolIdentityService identity,
        PrekeyStateService prekeys,
        DeviceListService devices)
    {
        _storage = storage;
        _identity = identity;
        _prekeys = prekeys;
        _devices = devices;
    }

    public async Task<SignedRoutingDescriptor> CreateAsync(
        IReadOnlyCollection<OwnedMailboxRoute> routes,
        SignedRoutingDescriptor? previous = null,
        CancellationToken cancellationToken = default)
    {
        ProtocolIdentity identity = await _identity.GetOrCreateAsync(cancellationToken);
        PrekeyPublication prekeys = await _prekeys.GetOrCreatePublicationAsync(cancellationToken: cancellationToken);
        SignedDeviceList deviceList = await _devices.GetOrCreateAsync(cancellationToken);
        if (previous is not null)
        {
            if (!Verify(previous) || previous.Descriptor.UserId != identity.UserId)
                throw new CryptographicException("Previous routing descriptor is invalid");
            if (previous.Descriptor.DeviceList.Document.Sequence > deviceList.Document.Sequence)
            {
                await _devices.ApplyAsync(previous.Descriptor.DeviceList, cancellationToken);
                deviceList = await _devices.GetOrCreateAsync(cancellationToken);
            }
            else if (previous.Descriptor.DeviceList.Document.Sequence == deviceList.Document.Sequence
                     && previous.Descriptor.DeviceList.Signature != deviceList.Signature)
            {
                throw new CryptographicException("Conflicting DeviceList fork detected in routing");
            }
        }
        if (!deviceList.Document.Devices.Any(value => value.DeviceId == identity.DeviceId))
            throw new CryptographicException("This device was revoked and cannot publish routing");
        await _gate.WaitAsync(cancellationToken);
        try
        {
            long sequence = Math.Max(
                                await ReadSequenceAsync(cancellationToken),
                                previous?.Descriptor.Sequence ?? 0) + 1;
            long now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
            long expiry = routes.Count == 0
                ? DateTimeOffset.UtcNow.AddDays(7).ToUnixTimeMilliseconds()
                : Math.Min(
                    DateTimeOffset.UtcNow.AddDays(30).ToUnixTimeMilliseconds(),
                    routes.Min(route => route.ExpiresAt.ToUnixTimeMilliseconds()));
            var devices = previous?.Descriptor.Devices
                              .Where(value => value.Identity.DeviceId != identity.DeviceId
                                              && deviceList.Document.Devices.Any(active =>
                                                  active.DeviceId == value.Identity.DeviceId))
                              .ToList()
                          ?? [];
            devices.Add(new DeviceRoutingEntry(
                identity,
                routes.Select(ToPublic).ToList(),
                prekeys.SignedPrekey.Descriptor.Sequence));
            var descriptor = new RoutingDescriptor(
                2,
                identity.UserId,
                sequence,
                now,
                expiry,
                deviceList,
                devices.OrderBy(value => value.Identity.DeviceId, StringComparer.Ordinal).ToList());
            string descriptorJson = JsonSerializer.Serialize(descriptor, JsonOptions);
            string signature = _identity.SignDeviceData(Encoding.UTF8.GetBytes(descriptorJson));
            await WriteSequenceAsync(sequence, cancellationToken);
            return new SignedRoutingDescriptor(descriptor, descriptorJson, signature);
        }
        finally
        {
            _gate.Release();
        }
    }

    public static bool Verify(SignedRoutingDescriptor signed)
    {
        if (signed.Descriptor.Version != 2
            || signed.Descriptor.Devices.Count == 0
            || !DeviceListService.Verify(signed.Descriptor.DeviceList)
            || signed.Descriptor.DeviceList.Document.UserId != signed.Descriptor.UserId
            || signed.Descriptor.ExpiresAtUnixMilliseconds <= DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
            || !string.Equals(
                signed.DescriptorJson,
                JsonSerializer.Serialize(signed.Descriptor, JsonOptions),
                StringComparison.Ordinal))
        {
            return false;
        }
        ProtocolIdentity identity = signed.Descriptor.Devices[0].Identity;
        if (identity.UserId != signed.Descriptor.UserId
            || signed.Descriptor.Devices.Any(device =>
                device.Identity.UserId != signed.Descriptor.UserId
                || !signed.Descriptor.DeviceList.Document.Devices.Any(active => active.DeviceId == device.Identity.DeviceId)
                || !ProtocolIdentityService.VerifyDeviceCertificate(device.Identity)))
        {
            return false;
        }
        byte[] signedBytes = Encoding.UTF8.GetBytes(signed.DescriptorJson);
        return signed.Descriptor.Devices.Any(device =>
                   ProtocolIdentityService.VerifyDeviceData(device.Identity, signedBytes, signed.Signature))
               || ProtocolIdentityService.VerifyIdentityData(identity, signedBytes, signed.Signature);
    }

    private async Task<long> ReadSequenceAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(SequencePath)) return 0;
        byte[] protectedBytes = await File.ReadAllBytesAsync(SequencePath, cancellationToken);
        byte[] plaintext = _storage.Unprotect(protectedBytes, SequencePurpose);
        return long.TryParse(Encoding.ASCII.GetString(plaintext), out long value) ? value : 0;
    }

    private async Task WriteSequenceAsync(long sequence, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(SequencePath)!);
        byte[] protectedBytes = _storage.Protect(
            Encoding.ASCII.GetBytes(sequence.ToString(System.Globalization.CultureInfo.InvariantCulture)),
            SequencePurpose);
        string temporary = SequencePath + ".new";
        await File.WriteAllBytesAsync(temporary, protectedBytes, cancellationToken);
        File.Move(temporary, SequencePath, overwrite: true);
    }

    private string SequencePath => Path.Combine(
        _storage.AppDirectory,
        "local-first",
        "routing-sequence-v2.secure");

    private static PublicMailboxRoute ToPublic(OwnedMailboxRoute route) => new(
        route.Node.NodeId,
        route.Node.BaseUrl,
        route.Node.PublicKey,
        route.MailboxId,
        route.DeviceHint,
        route.ContactCapability,
        route.ExpiresAt,
        route.Node.ContactPowBits,
        route.Node.Transports);
}
