using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.LocalFirst;

namespace TuratText.Client.Transport.V2;

public sealed record PortableEnvelopeEntry(PublicMailboxRoute Route, MailboxEnvelope Envelope);

public sealed record PortableEnvelopeBundle(
    int Version,
    string BundleId,
    long CreatedAtUnixMilliseconds,
    long ExpiresAtUnixMilliseconds,
    ProtocolIdentity Sender,
    IReadOnlyList<PortableEnvelopeEntry> Entries,
    string Signature);

public sealed class PortableEnvelopeService
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web) { WriteIndented = true };
    private readonly LocalFirstRuntime _runtime;
    private readonly ProtocolIdentityService _identity;
    private readonly IMailboxTransport _transport;

    public PortableEnvelopeService(
        LocalFirstRuntime runtime,
        ProtocolIdentityService identity,
        IMailboxTransport transport)
    {
        _runtime = runtime;
        _identity = identity;
        _transport = transport;
    }

    public async Task<int> ExportDueOutboxAsync(
        string destinationPath,
        CancellationToken cancellationToken = default)
    {
        IReadOnlyList<LocalDeliveryJob> jobs = await _runtime.Events.ReadDueDeliveriesAsync(
            1000,
            cancellationToken);
        var entries = new List<PortableEnvelopeEntry>();
        foreach (LocalDeliveryJob job in jobs)
        {
            DeliveryJobPayload? payload = JsonSerializer.Deserialize<DeliveryJobPayload>(job.Payload, JsonOptions);
            if (payload is not null) entries.Add(new PortableEnvelopeEntry(payload.Route, payload.Envelope));
        }
        if (entries.Count == 0) return 0;
        long created = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        long expiry = entries.Min(value => value.Envelope.ExpiresAt.ToUnixTimeMilliseconds());
        var unsigned = new PortableEnvelopeBundle(
            2,
            "mesh1-" + RandomToken(18),
            created,
            expiry,
            _identity.Current!,
            entries,
            "");
        string signature = _identity.SignDeviceData(SigningBytes(unsigned));
        string temporary = destinationPath + ".new";
        Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(destinationPath))!);
        await File.WriteAllTextAsync(
            temporary,
            JsonSerializer.Serialize(unsigned with { Signature = signature }, JsonOptions),
            cancellationToken);
        File.Move(temporary, destinationPath, overwrite: true);
        return entries.Count;
    }

    public async Task<int> ForwardAsync(string sourcePath, CancellationToken cancellationToken = default)
    {
        PortableEnvelopeBundle bundle = JsonSerializer.Deserialize<PortableEnvelopeBundle>(
                                             await File.ReadAllTextAsync(sourcePath, cancellationToken),
                                             JsonOptions)
                                         ?? throw new CryptographicException("Portable envelope bundle is invalid");
        if (!Verify(bundle) || bundle.ExpiresAtUnixMilliseconds <= DateTimeOffset.UtcNow.ToUnixTimeMilliseconds())
            throw new CryptographicException("Portable envelope bundle is expired or invalid");
        int forwarded = 0;
        foreach (PortableEnvelopeEntry entry in bundle.Entries)
        {
            await _transport.PutAsync(entry.Route, entry.Envelope, cancellationToken);
            forwarded++;
        }
        return forwarded;
    }

    public static bool Verify(PortableEnvelopeBundle value)
    {
        try
        {
            return value.Version == 2
                   && value.Entries.Count is > 0 and <= 1000
                   && value.Entries.All(entry => entry.Envelope.ExpiresAt.ToUnixTimeMilliseconds()
                       >= value.ExpiresAtUnixMilliseconds)
                   && ProtocolIdentityService.VerifyDeviceData(value.Sender, SigningBytes(value), value.Signature);
        }
        catch
        {
            return false;
        }
    }

    private static byte[] SigningBytes(PortableEnvelopeBundle value) => Encoding.UTF8.GetBytes(
        JsonSerializer.Serialize(value with { Signature = "" }, JsonOptions));

    private static string RandomToken(int bytes) => Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
        .TrimEnd('=').Replace('+', '-').Replace('/', '_');

    private sealed record DeliveryJobPayload(PublicMailboxRoute Route, MailboxEnvelope Envelope);
}
