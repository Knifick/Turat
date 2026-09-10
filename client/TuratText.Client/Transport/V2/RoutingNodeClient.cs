using System.Net.Http.Json;
using System.Text.Json;

namespace TuratText.Client.Transport.V2;

public sealed class RoutingNodeClient
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;

    public RoutingNodeClient(HttpClient http)
    {
        _http = http;
    }

    public async Task PublishAsync(
        NodeDescriptor node,
        SignedRoutingDescriptor descriptor,
        CancellationToken cancellationToken = default)
    {
        if (!NodeDescriptorVerifier.Verify(node) || !RoutingDescriptorService.Verify(descriptor))
        {
            throw new System.Security.Cryptography.CryptographicException("Routing or node signature is invalid");
        }
        using HttpResponseMessage response = await _http.PutAsJsonAsync(
            BuildUri(node.BaseUrl, $"v2/routing/{Uri.EscapeDataString(descriptor.Descriptor.UserId)}"),
            new
            {
                identityPublicKey = descriptor.Descriptor.Devices[0].Identity.IdentityPublicKey,
                sequence = descriptor.Descriptor.Sequence,
                descriptorJson = descriptor.DescriptorJson,
                signature = descriptor.Signature,
                expiresAt = DateTimeOffset.FromUnixTimeMilliseconds(descriptor.Descriptor.ExpiresAtUnixMilliseconds)
            },
            JsonOptions,
            cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
    }

    public async Task<SignedRoutingDescriptor> GetAsync(
        NodeDescriptor node,
        string userId,
        CancellationToken cancellationToken = default)
    {
        using HttpResponseMessage response = await _http.GetAsync(
            BuildUri(node.BaseUrl, $"v2/routing/{Uri.EscapeDataString(userId)}"),
            cancellationToken);
        await EnsureSuccessAsync(response, cancellationToken);
        RoutingWire wire = await response.Content.ReadFromJsonAsync<RoutingWire>(JsonOptions, cancellationToken)
                           ?? throw new InvalidOperationException("Node returned an empty routing record");
        RoutingDescriptor descriptor = JsonSerializer.Deserialize<RoutingDescriptor>(wire.DescriptorJson, JsonOptions)
                                       ?? throw new InvalidOperationException("Routing descriptor is invalid");
        var signed = new SignedRoutingDescriptor(descriptor, wire.DescriptorJson, wire.Signature);
        if (wire.UserId != userId || wire.Sequence != descriptor.Sequence || !RoutingDescriptorService.Verify(signed))
        {
            throw new System.Security.Cryptography.CryptographicException("Routing descriptor signature is invalid");
        }
        return signed;
    }

    private static Uri BuildUri(string baseUrl, string path) => new(new Uri(baseUrl.TrimEnd('/') + "/"), path);

    private static async Task EnsureSuccessAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        if (response.IsSuccessStatusCode) return;
        string details = await response.Content.ReadAsStringAsync(cancellationToken);
        throw new HttpRequestException($"Routing node request failed with HTTP {(int)response.StatusCode}: {details}");
    }

    private sealed record RoutingWire(
        string UserId,
        string IdentityPublicKey,
        long Sequence,
        string DescriptorJson,
        string Signature,
        DateTimeOffset ExpiresAt,
        DateTimeOffset UpdatedAt);
}
