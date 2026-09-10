using System.Net;
using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.LocalFirst;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed record ProfileClaimDocument(
    int Version,
    string UserId,
    long Sequence,
    string DisplayName,
    string About,
    string? AvatarMimeType,
    string? AvatarBase64,
    long ExpiresAtUnixMilliseconds);

public sealed record SignedProfileClaim(
    ProfileClaimDocument Claim,
    string ClaimJson,
    string IdentityPublicKey,
    string Signature);

public sealed class ProfileDirectoryClient
{
    public const int MaxDisplayNameLength = 64;
    public const int MaxAboutLength = 200;
    private const string SequencePurpose = "TuratText.ProfileSequence.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;
    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public ProfileDirectoryClient(
        HttpClient http,
        IProtectedStorage storage,
        ProtocolIdentityService identity)
    {
        _http = http;
        _storage = storage;
        _identity = identity;
    }

    public async Task<SignedProfileClaim> PublishAsync(
        IEnumerable<NodeDescriptor> nodes,
        string displayName,
        string about,
        string? avatarMimeType,
        string? avatarBase64,
        CancellationToken cancellationToken = default)
    {
        displayName = (displayName ?? string.Empty).Trim();
        about = (about ?? string.Empty).Trim();
        if (displayName.Length is 0 or > MaxDisplayNameLength)
            throw new ArgumentException($"Display name must be 1-{MaxDisplayNameLength} characters");
        if (about.Length > MaxAboutLength)
            throw new ArgumentException($"About must be at most {MaxAboutLength} characters");

        ProtocolIdentity identity = await _identity.GetOrCreateAsync(cancellationToken);
        if (!_identity.HasIdentityAuthority)
            throw new InvalidOperationException(
                "Profile can only be changed by the identity-authority device");
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
                ProfileWire? current = await TryGetByUserAsync(node, identity.UserId, cancellationToken);
                if (current is not null) sequence = Math.Max(sequence, current.Sequence);
            }
            sequence = checked(sequence + 1);
            long expiry = DateTimeOffset.UtcNow.AddDays(30).ToUnixTimeMilliseconds();
            var claim = new ProfileClaimDocument(
                1, identity.UserId, sequence, displayName, about, avatarMimeType, avatarBase64, expiry);
            string claimJson = JsonSerializer.Serialize(claim, JsonOptions);
            var signed = new SignedProfileClaim(
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
                        BuildUri(node.BaseUrl, $"v2/profiles/{Uri.EscapeDataString(identity.UserId)}"),
                        new
                        {
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
            if (published == 0) throw new HttpRequestException("Profile claim was rejected by every node", lastError);
            await WriteSequenceAsync(sequence, cancellationToken);
            await CacheLocalAsync(signed, cancellationToken);
            return signed;
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<SignedProfileClaim?> ResolveAsync(
        IEnumerable<NodeDescriptor> nodes,
        string userId,
        CancellationToken cancellationToken = default)
    {
        SignedProfileClaim? best = null;
        foreach (NodeDescriptor node in nodes.Where(value => NodeDescriptorVerifier.Verify(value)).DistinctBy(value => value.NodeId))
        {
            try
            {
                ProfileWire? wire = await TryGetByUserAsync(node, userId, cancellationToken);
                if (wire is null) continue;
                SignedProfileClaim signed = FromWire(wire);
                if (best is null || signed.Claim.Sequence > best.Claim.Sequence) best = signed;
            }
            catch (Exception exception) when (exception is HttpRequestException or JsonException or CryptographicException)
            {
                // Discovery is multi-source: an unavailable or malicious source does not block the others.
            }
        }
        return best;
    }

    public async Task<SignedProfileClaim?> ReadCachedOwnProfileAsync(CancellationToken cancellationToken = default)
    {
        if (!File.Exists(CachePath)) return null;
        try
        {
            byte[] protectedValue = await File.ReadAllBytesAsync(CachePath, cancellationToken);
            string json = Encoding.UTF8.GetString(_storage.Unprotect(protectedValue, CachePurpose));
            ProfileWire? wire = JsonSerializer.Deserialize<ProfileWire>(json, JsonOptions);
            return wire is null ? null : FromWire(wire);
        }
        catch
        {
            return null;
        }
    }

    public static bool Verify(SignedProfileClaim signed)
    {
        try
        {
            if (signed.Claim.Version != 1
                || signed.Claim.ExpiresAtUnixMilliseconds <= DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()
                || signed.Claim.DisplayName.Length is 0 or > MaxDisplayNameLength
                || signed.Claim.About.Length > MaxAboutLength
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

    private async Task<ProfileWire?> TryGetByUserAsync(
        NodeDescriptor node,
        string userId,
        CancellationToken cancellationToken)
    {
        using HttpResponseMessage response = await _http.GetAsync(
            BuildUri(node.BaseUrl, $"v2/profiles/{Uri.EscapeDataString(userId)}"),
            cancellationToken);
        if (response.StatusCode == HttpStatusCode.NotFound) return null;
        await EnsureSuccessAsync(response, cancellationToken);
        ProfileWire? wire = await response.Content.ReadFromJsonAsync<ProfileWire>(JsonOptions, cancellationToken);
        if (wire is null || !Verify(FromWire(wire)))
            throw new CryptographicException("Directory returned an invalid profile claim");
        return wire;
    }

    private static SignedProfileClaim FromWire(ProfileWire wire)
    {
        ProfileClaimDocument claim = JsonSerializer.Deserialize<ProfileClaimDocument>(wire.ClaimJson, JsonOptions)
                                      ?? throw new JsonException("Profile claim is empty");
        if (wire.UserId != claim.UserId || wire.Sequence != claim.Sequence)
            throw new CryptographicException("Profile record fields do not match its signed claim");
        return new SignedProfileClaim(claim, wire.ClaimJson, wire.IdentityPublicKey, wire.Signature);
    }

    private async Task CacheLocalAsync(SignedProfileClaim signed, CancellationToken cancellationToken)
    {
        var wire = new ProfileWire(
            signed.Claim.UserId, signed.IdentityPublicKey, signed.Claim.Sequence,
            signed.ClaimJson, signed.Signature,
            DateTimeOffset.FromUnixTimeMilliseconds(signed.Claim.ExpiresAtUnixMilliseconds), DateTimeOffset.UtcNow);
        Directory.CreateDirectory(Path.GetDirectoryName(CachePath)!);
        byte[] protectedValue = _storage.Protect(
            Encoding.UTF8.GetBytes(JsonSerializer.Serialize(wire, JsonOptions)), CachePurpose);
        string temporary = CachePath + ".new";
        await File.WriteAllBytesAsync(temporary, protectedValue, cancellationToken);
        File.Move(temporary, CachePath, true);
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

    private string SequencePath => Path.Combine(_storage.AppDirectory, "local-first", "profile-sequence-v2.secure");
    private string CachePath => Path.Combine(_storage.AppDirectory, "local-first", "profile-own-v2.secure");
    private const string CachePurpose = "TuratText.ProfileOwnCache.v2";

    private static Uri BuildUri(string baseUrl, string path) => new(new Uri(baseUrl.TrimEnd('/') + "/"), path);

    private static async Task EnsureSuccessAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        if (response.IsSuccessStatusCode) return;
        string details = await response.Content.ReadAsStringAsync(cancellationToken);
        throw new HttpRequestException(
            $"Profile directory request failed with HTTP {(int)response.StatusCode}: {details}",
            null,
            response.StatusCode);
    }

    private sealed record ProfileWire(
        string UserId,
        string IdentityPublicKey,
        long Sequence,
        string ClaimJson,
        string Signature,
        DateTimeOffset ExpiresAt,
        DateTimeOffset UpdatedAt);
}
