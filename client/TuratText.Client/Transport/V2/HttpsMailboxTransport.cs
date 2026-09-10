using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace TuratText.Client.Transport.V2;

public sealed class HttpsMailboxTransport : IMailboxTransport, IDisposable
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;
    private readonly bool _ownsClient;

    public HttpsMailboxTransport(HttpClient? httpClient = null)
    {
        _http = httpClient ?? new HttpClient { Timeout = TimeSpan.FromSeconds(20) };
        _ownsClient = httpClient is null;
    }

    public async Task<NodeDescriptor> GetNodeDescriptorAsync(
        Uri bootstrapUri,
        string? expectedNodeId = null,
        CancellationToken cancellationToken = default)
    {
        Uri uri = BuildUri(bootstrapUri.ToString(), "v2/node-descriptor");
        NodeDescriptor? descriptor = await _http.GetFromJsonAsync<NodeDescriptor>(uri, JsonOptions, cancellationToken);
        if (descriptor is null || !NodeDescriptorVerifier.Verify(descriptor, expectedNodeId))
        {
            throw new CryptographicException("Node descriptor signature is invalid");
        }
        Uri declared = new(descriptor.BaseUrl);
        if (!IsSecureOrLoopback(declared))
        {
            throw new InvalidOperationException("Mailbox nodes must use HTTPS outside localhost");
        }
        return descriptor;
    }

    public async Task<OwnedMailboxRoute> RegisterMailboxAsync(
        NodeDescriptor node,
        string deviceHint,
        CancellationToken cancellationToken = default)
    {
        if (!NodeDescriptorVerifier.Verify(node))
        {
            throw new CryptographicException("Node descriptor signature is invalid");
        }
        string readCapability = RandomToken(32);
        string writeCapability = RandomToken(32);
        string contactCapability = RandomToken(32);
        string proofNonce = await FindProofOfWorkAsync(
            readCapability,
            writeCapability,
            contactCapability,
            node.RegistrationPowBits,
            cancellationToken);
        DateTimeOffset expiresAt = DateTimeOffset.UtcNow.AddHours(Math.Max(1, node.MaxMailboxTtlHours));
        using HttpResponseMessage response = await _http.PostAsJsonAsync(
            BuildUri(node.BaseUrl, "v2/mailboxes"),
            new
            {
                readCapability,
                writeCapability,
                contactCapability,
                deviceHint,
                expiresAt,
                proofNonce
            },
            JsonOptions,
            cancellationToken);
        MailboxRegistration registration = await ReadAsync<MailboxRegistration>(response, cancellationToken);
        return new OwnedMailboxRoute(
            node,
            registration.MailboxId,
            registration.DeviceHint,
            readCapability,
            writeCapability,
            contactCapability,
            registration.CreatedAt,
            registration.ExpiresAt);
    }

    public async Task PutAsync(
        PublicMailboxRoute route,
        MailboxEnvelope envelope,
        CancellationToken cancellationToken = default)
    {
        using var request = new HttpRequestMessage(
            HttpMethod.Put,
            BuildUri(route.BaseUrl, $"v2/mailboxes/{route.MailboxId}/envelopes"));
        request.Headers.Add("X-Mailbox-Write-Capability", route.WriteCapability);
        request.Headers.Add("X-Envelope-Pow-Nonce", await FindEnvelopeProofOfWorkAsync(
            route,
            envelope,
            cancellationToken));
        request.Content = JsonContent.Create(envelope, options: JsonOptions);
        using HttpResponseMessage response = await _http.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public async Task<IReadOnlyList<MailboxEnvelope>> FetchAsync(
        OwnedMailboxRoute route,
        int limit = 100,
        CancellationToken cancellationToken = default)
    {
        using var request = new HttpRequestMessage(
            HttpMethod.Get,
            BuildUri(route.Node.BaseUrl, $"v2/mailboxes/{route.MailboxId}/envelopes?limit={Math.Clamp(limit, 1, 200)}"));
        request.Headers.Add("X-Mailbox-Read-Capability", route.ReadCapability);
        using HttpResponseMessage response = await _http.SendAsync(request, cancellationToken);
        return await ReadAsync<IReadOnlyList<MailboxEnvelope>>(response, cancellationToken);
    }

    public async Task AcknowledgeAsync(
        OwnedMailboxRoute route,
        IReadOnlyCollection<string> envelopeIds,
        CancellationToken cancellationToken = default)
    {
        if (envelopeIds.Count == 0) return;
        using var request = new HttpRequestMessage(
            HttpMethod.Post,
            BuildUri(route.Node.BaseUrl, $"v2/mailboxes/{route.MailboxId}/ack"));
        request.Headers.Add("X-Mailbox-Read-Capability", route.ReadCapability);
        request.Content = JsonContent.Create(new { envelopeIds }, options: JsonOptions);
        using HttpResponseMessage response = await _http.SendAsync(request, cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public void Dispose()
    {
        if (_ownsClient) _http.Dispose();
    }

    private static async Task<string> FindProofOfWorkAsync(
        string readCapability,
        string writeCapability,
        string contactCapability,
        int bits,
        CancellationToken cancellationToken)
    {
        if (bits <= 0) return "0";
        for (long nonce = 0; ; nonce++)
        {
            if ((nonce & 8191) == 0)
            {
                cancellationToken.ThrowIfCancellationRequested();
                await Task.Yield();
            }
            string candidate = nonce.ToString(System.Globalization.CultureInfo.InvariantCulture);
            byte[] digest = SHA256.HashData(Encoding.UTF8.GetBytes(
                readCapability + ":" + writeCapability + ":" + contactCapability + ":" + candidate));
            if (LeadingZeroBits(digest) >= bits) return candidate;
        }
    }

    private static int LeadingZeroBits(byte[] value)
    {
        int count = 0;
        foreach (byte item in value)
        {
            if (item == 0)
            {
                count += 8;
                continue;
            }
            count += System.Numerics.BitOperations.LeadingZeroCount(item) - 24;
            break;
        }
        return count;
    }

    private static async Task<string> FindEnvelopeProofOfWorkAsync(
        PublicMailboxRoute route,
        MailboxEnvelope envelope,
        CancellationToken cancellationToken)
    {
        if (route.EnvelopePowBits <= 0) return "0";
        string payloadHash = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(envelope.OpaquePayload)))
            .ToLowerInvariant();
        string prefix = route.MailboxId + ":" + envelope.EnvelopeId + ":" + payloadHash + ":";
        for (long nonce = 0; ; nonce++)
        {
            if ((nonce & 8191) == 0)
            {
                cancellationToken.ThrowIfCancellationRequested();
                await Task.Yield();
            }
            string candidate = nonce.ToString(System.Globalization.CultureInfo.InvariantCulture);
            if (LeadingZeroBits(SHA256.HashData(Encoding.UTF8.GetBytes(prefix + candidate))) >= route.EnvelopePowBits)
                return candidate;
        }
    }

    private static async Task<T> ReadAsync<T>(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        await EnsureSuccessAsync(response, cancellationToken);
        return await response.Content.ReadFromJsonAsync<T>(JsonOptions, cancellationToken)
               ?? throw new InvalidOperationException("Node returned an empty response");
    }

    private static async Task EnsureSuccessAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        if (response.IsSuccessStatusCode) return;
        string details = await response.Content.ReadAsStringAsync(cancellationToken);
        throw new HttpRequestException(
            $"Node request failed with HTTP {(int)response.StatusCode}: {details}",
            null,
            response.StatusCode);
    }

    private static Uri BuildUri(string baseUrl, string path) =>
        new(new Uri(baseUrl.TrimEnd('/') + "/"), path);

    private static bool IsSecureOrLoopback(Uri uri) =>
        uri.Scheme.Equals(Uri.UriSchemeHttps, StringComparison.OrdinalIgnoreCase)
        || uri.IsLoopback;

    private static string RandomToken(int bytes) =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
            .TrimEnd('=')
            .Replace('+', '-')
            .Replace('/', '_');

    private sealed record MailboxRegistration(
        Guid MailboxId,
        string DeviceHint,
        DateTimeOffset CreatedAt,
        DateTimeOffset ExpiresAt);
}
