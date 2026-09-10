using System.Net;
using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.LocalFirst;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed record UsernameClaimDocument(
    int Version,
    string Username,
    string UserId,
    long Sequence,
    long ExpiresAtUnixMilliseconds);

public sealed record SignedUsernameClaim(
    UsernameClaimDocument Claim,
    string ClaimJson,
    string IdentityPublicKey,
    string Signature);

public sealed class UsernameDirectoryClient
{
    private const string SequencePurpose = "TuratText.UsernameSequence.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;
    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public UsernameDirectoryClient(
        HttpClient http,
        IProtectedStorage storage,
        ProtocolIdentityService identity)
    {
        _http = http;
        _storage = storage;
        _identity = identity;
    }

    public async Task<SignedUsernameClaim> PublishAsync(
        IEnumerable<NodeDescriptor> nodes,
        string username,
        CancellationToken cancellationToken = default)
    {
        string normalized = Normalize(username);
        ProtocolIdentity identity = await _identity.GetOrCreateAsync(cancellationToken);
        if (!_identity.HasIdentityAuthority)
            throw new InvalidOperationException(
                "Username and device revocations can only be changed by the identity-authority device");
        List<NodeDescriptor> verifiedNodes = nodes
            .Where(value => NodeDescriptorVerifier.Verify(value))
            .DistinctBy(value => value.NodeId)
            .ToList();
        if (verifiedNodes.Count == 0) throw new InvalidOperationException("No verified directory nodes are available");

        await _gate.WaitAsync(cancellationToken);
        try
        {
            long sequence = await ReadSequenceAsync(cancellationToken);
            foreach (NodeDescriptor node in verifiedNodes)
            {
                UsernameWire? current = await TryGetByUserAsync(node, identity.UserId, cancellationToken);
                if (current is not null) sequence = Math.Max(sequence, current.Sequence);
            }
            sequence = checked(sequence + 1);
            long expiry = DateTimeOffset.UtcNow.AddDays(30).ToUnixTimeMilliseconds();
            var claim = new UsernameClaimDocument(2, normalized, identity.UserId, sequence, expiry);
            string claimJson = JsonSerializer.Serialize(claim, JsonOptions);
            var signed = new SignedUsernameClaim(
                claim,
                claimJson,
                identity.IdentityPublicKey,
                _identity.SignIdentityData(Encoding.UTF8.GetBytes(claimJson)));
            int published = 0;
            Exception? lastError = null;
            foreach (NodeDescriptor node in verifiedNodes)
            {
                try
                {
                    using HttpResponseMessage response = await _http.PutAsJsonAsync(
                        BuildUri(node.BaseUrl, $"v2/usernames/{Uri.EscapeDataString(normalized)}"),
                        new
                        {
                            userId = identity.UserId,
                            identityPublicKey = identity.IdentityPublicKey,
                            sequence,
                            claimJson,
                            signature = signed.Signature,
                            expiresAt = DateTimeOffset.FromUnixTimeMilliseconds(expiry)
                        },
                        JsonOptions,
                        cancellationToken);
                    await EnsureSuccessAsync(response, cancellationToken);
                    published++;
                }
                catch (Exception exception) when (exception is not OperationCanceledException)
                {
                    lastError = exception;
                }
            }
            if (published == 0) throw new HttpRequestException("Username claim was rejected by every node", lastError);
            await WriteSequenceAsync(sequence, cancellationToken);
            return signed;
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<IReadOnlyList<SignedUsernameClaim>> ResolveAsync(
        IEnumerable<NodeDescriptor> nodes,
        string username,
        CancellationToken cancellationToken = default)
    {
        string normalized = Normalize(username);
        var claims = new Dictionary<string, SignedUsernameClaim>(StringComparer.Ordinal);
        foreach (NodeDescriptor node in nodes.Where(value => NodeDescriptorVerifier.Verify(value)).DistinctBy(value => value.NodeId))
        {
            try
            {
                using HttpResponseMessage response = await _http.GetAsync(
                    BuildUri(node.BaseUrl, $"v2/usernames/{Uri.EscapeDataString(normalized)}"),
                    cancellationToken);
                await EnsureSuccessAsync(response, cancellationToken);
                IReadOnlyList<UsernameWire> records =
                    await response.Content.ReadFromJsonAsync<IReadOnlyList<UsernameWire>>(JsonOptions, cancellationToken)
                    ?? [];
                foreach (UsernameWire record in records)
                {
                    SignedUsernameClaim signed = FromWire(record);
                    if (!Verify(signed) || signed.Claim.Username != normalized) continue;
                    if (!claims.TryGetValue(signed.Claim.UserId, out SignedUsernameClaim? existing)
                        || signed.Claim.Sequence > existing.Claim.Sequence)
                    {
                        claims[signed.Claim.UserId] = signed;
                    }
                }
            }
            catch (Exception exception) when (exception is HttpRequestException or JsonException)
            {
                // Discovery is multi-source: an unavailable or malicious source does not block the others.
            }
        }
        return claims.Values.OrderBy(value => value.Claim.UserId, StringComparer.Ordinal).ToList();
    }

    /// <summary>Reverse lookup used to show a contact's published username, e.g. in a profile view.</summary>
    public async Task<SignedUsernameClaim?> ResolveByUserIdAsync(
        IEnumerable<NodeDescriptor> nodes,
        string userId,
        CancellationToken cancellationToken = default)
    {
        foreach (NodeDescriptor node in nodes.Where(value => NodeDescriptorVerifier.Verify(value)).DistinctBy(value => value.NodeId))
        {
            try
            {
                UsernameWire? wire = await TryGetByUserAsync(node, userId, cancellationToken);
                if (wire is not null) return FromWire(wire);
            }
            catch (Exception exception) when (exception is HttpRequestException or JsonException or CryptographicException)
            {
                // Discovery is multi-source: an unavailable or malicious source does not block the others.
            }
        }
        return null;
    }

    public static bool Verify(SignedUsernameClaim signed)
    {
        try
        {
            if (signed.Claim.Version != 2
                || signed.Claim.Username != Normalize(signed.Claim.Username)
                || signed.Claim.ExpiresAtUnixMilliseconds <= DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
                || !string.Equals(
                    signed.ClaimJson,
                    JsonSerializer.Serialize(signed.Claim, JsonOptions),
                    StringComparison.Ordinal))
            {
                return false;
            }
            byte[] publicKey = Convert.FromBase64String(signed.IdentityPublicKey);
            string userId = "tt1-" + Convert.ToHexString(SHA256.HashData(publicKey)).ToLowerInvariant();
            if (userId != signed.Claim.UserId) return false;
            using ECDsa verifier = ECDsa.Create();
            verifier.ImportSubjectPublicKeyInfo(publicKey, out _);
            return verifier.VerifyData(
                Encoding.UTF8.GetBytes(signed.ClaimJson),
                Convert.FromBase64String(signed.Signature),
                HashAlgorithmName.SHA256,
                DSASignatureFormat.IeeeP1363FixedFieldConcatenation);
        }
        catch
        {
            return false;
        }
    }

    public static string Normalize(string value)
    {
        string normalized = value.Trim().TrimStart('@').ToLowerInvariant();
        if (normalized.Length is < 3 or > 32
            || !char.IsAsciiLetterOrDigit(normalized[0])
            || normalized.Any(character => !char.IsAsciiLetterOrDigit(character)
                                           && character is not '_' and not '-' and not '.'))
        {
            throw new ArgumentException("Username must contain 3-32 ASCII letters, digits, '.', '_' or '-'");
        }
        return normalized;
    }

    private async Task<UsernameWire?> TryGetByUserAsync(
        NodeDescriptor node,
        string userId,
        CancellationToken cancellationToken)
    {
        using HttpResponseMessage response = await _http.GetAsync(
            BuildUri(node.BaseUrl, $"v2/usernames/by-user/{Uri.EscapeDataString(userId)}"),
            cancellationToken);
        if (response.StatusCode == HttpStatusCode.NotFound) return null;
        await EnsureSuccessAsync(response, cancellationToken);
        UsernameWire? wire = await response.Content.ReadFromJsonAsync<UsernameWire>(JsonOptions, cancellationToken);
        if (wire is null || !Verify(FromWire(wire)))
            throw new CryptographicException("Directory returned an invalid username claim");
        return wire;
    }

    private static SignedUsernameClaim FromWire(UsernameWire wire)
    {
        UsernameClaimDocument claim = JsonSerializer.Deserialize<UsernameClaimDocument>(wire.ClaimJson, JsonOptions)
                                      ?? throw new JsonException("Username claim is empty");
        if (wire.Username != claim.Username || wire.UserId != claim.UserId || wire.Sequence != claim.Sequence)
            throw new CryptographicException("Username record fields do not match its signed claim");
        return new SignedUsernameClaim(claim, wire.ClaimJson, wire.IdentityPublicKey, wire.Signature);
    }

    private async Task<long> ReadSequenceAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(SequencePath)) return 0;
        byte[] protectedValue = await File.ReadAllBytesAsync(SequencePath, cancellationToken);
        string value = Encoding.ASCII.GetString(_storage.Unprotect(protectedValue, SequencePurpose));
        return long.TryParse(value, out long sequence) ? sequence : 0;
    }

    private async Task WriteSequenceAsync(long sequence, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(SequencePath)!);
        byte[] protectedValue = _storage.Protect(
            Encoding.ASCII.GetBytes(sequence.ToString(System.Globalization.CultureInfo.InvariantCulture)),
            SequencePurpose);
        string temporary = SequencePath + ".new";
        await File.WriteAllBytesAsync(temporary, protectedValue, cancellationToken);
        File.Move(temporary, SequencePath, true);
    }

    private string SequencePath => Path.Combine(_storage.AppDirectory, "local-first", "username-sequence-v2.secure");

    private static Uri BuildUri(string baseUrl, string path) => new(new Uri(baseUrl.TrimEnd('/') + "/"), path);

    private static async Task EnsureSuccessAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        if (response.IsSuccessStatusCode) return;
        string details = await response.Content.ReadAsStringAsync(cancellationToken);
        throw new HttpRequestException(
            $"Username directory request failed with HTTP {(int)response.StatusCode}: {details}",
            null,
            response.StatusCode);
    }

    private sealed record UsernameWire(
        string Username,
        string UserId,
        string IdentityPublicKey,
        long Sequence,
        string ClaimJson,
        string Signature,
        DateTimeOffset ExpiresAt,
        DateTimeOffset UpdatedAt);
}
