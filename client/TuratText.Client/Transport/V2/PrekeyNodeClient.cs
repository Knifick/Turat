using System.Net.Http.Json;
using System.Text.Json;
using TuratText.Client.Crypto.V2;
using TuratText.Client.LocalFirst;

namespace TuratText.Client.Transport.V2;

public sealed class PrekeyNodeClient
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;

    public PrekeyNodeClient(HttpClient http)
    {
        _http = http;
    }

    public async Task PublishAsync(
        OwnedMailboxRoute mailbox,
        PrekeyPublication publication,
        CancellationToken cancellationToken = default)
    {
        if (!PrekeyStateService.VerifyPublication(publication))
        {
            throw new InvalidOperationException("Local prekey publication is invalid");
        }
        using var request = new HttpRequestMessage(
            HttpMethod.Put,
            BuildUri(mailbox.Node.BaseUrl, $"v2/prekeys/{mailbox.MailboxId}"));
        request.Headers.Add("X-Mailbox-Write-Capability", mailbox.WriteCapability);
        request.Content = JsonContent.Create(new
        {
            userId = publication.Identity.UserId,
            deviceId = publication.Identity.DeviceId,
            identityJson = JsonSerializer.Serialize(publication.Identity, JsonOptions),
            signedPrekeyJson = JsonSerializer.Serialize(publication.SignedPrekey, JsonOptions),
            sequence = publication.SignedPrekey.Descriptor.Sequence,
            expiresAt = DateTimeOffset.FromUnixTimeMilliseconds(
                publication.SignedPrekey.Descriptor.ExpiresAtUnixMilliseconds),
            oneTimePrekeys = publication.OneTimePrekeys.Select(value => new
            {
                prekeyId = value.PrekeyId,
                prekeyJson = JsonSerializer.Serialize(value, JsonOptions)
            })
        }, options: JsonOptions);
        using HttpResponseMessage response = await _http.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public async Task<ClaimedPrekeyBundle> ClaimAsync(
        string baseUrl,
        string userId,
        string deviceId,
        CancellationToken cancellationToken = default)
    {
        using HttpResponseMessage response = await _http.GetAsync(
            BuildUri(baseUrl, $"v2/prekeys/{Uri.EscapeDataString(userId)}/{Uri.EscapeDataString(deviceId)}/claim"),
            cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        ClaimedResponse wire = await response.Content.ReadFromJsonAsync<ClaimedResponse>(JsonOptions, cancellationToken)
                               ?? throw new InvalidOperationException("Node returned an empty prekey bundle");
        ProtocolIdentity identity = JsonSerializer.Deserialize<ProtocolIdentity>(wire.IdentityJson, JsonOptions)
                                    ?? throw new InvalidOperationException("Prekey identity is invalid");
        SignedDevicePrekey signed = JsonSerializer.Deserialize<SignedDevicePrekey>(wire.SignedPrekeyJson, JsonOptions)
                                    ?? throw new InvalidOperationException("Signed prekey is invalid");
        OneTimePrekeyPublic? oneTime = wire.OneTimePrekey is null
            ? null
            : JsonSerializer.Deserialize<OneTimePrekeyPublic>(wire.OneTimePrekey.PrekeyJson, JsonOptions);
        var publication = new PrekeyPublication(identity, signed, oneTime is null ? [] : [oneTime]);
        if (!PrekeyStateService.VerifyPublication(publication)
            || wire.UserId != identity.UserId
            || wire.DeviceId != identity.DeviceId)
        {
            throw new System.Security.Cryptography.CryptographicException("Claimed prekey bundle signature is invalid");
        }
        return new ClaimedPrekeyBundle(
            wire.UserId,
            wire.DeviceId,
            identity,
            signed,
            oneTime,
            wire.Sequence,
            wire.ExpiresAt);
    }

    private static Uri BuildUri(string baseUrl, string path) => new(new Uri(baseUrl.TrimEnd('/') + "/"), path);

    private static async Task EnsureSuccessAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        if (response.IsSuccessStatusCode) return;
        string details = await response.Content.ReadAsStringAsync(cancellationToken);
        throw new HttpRequestException($"Prekey node request failed with HTTP {(int)response.StatusCode}: {details}");
    }

    private sealed record ClaimedResponse(
        string UserId,
        string DeviceId,
        string IdentityJson,
        string SignedPrekeyJson,
        OneTimeResponse? OneTimePrekey,
        long Sequence,
        DateTimeOffset ExpiresAt);

    private sealed record OneTimeResponse(string PrekeyId, string PrekeyJson);
}
