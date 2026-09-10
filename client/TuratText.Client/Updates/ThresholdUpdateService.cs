using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Net.Http.Json;
using Org.BouncyCastle.Crypto.Parameters;
using Org.BouncyCastle.Crypto.Signers;
using Org.BouncyCastle.Security;
using TuratText.Client.Services;

namespace TuratText.Client.Updates;

public sealed record UpdateArtifact(
    string Platform,
    string Architecture,
    string Url,
    long Size,
    string Sha256);

public sealed record UpdateManifestBody(
    int Version,
    string Channel,
    long Sequence,
    string ReleaseVersion,
    long PublishedAtUnixMilliseconds,
    long ExpiresAtUnixMilliseconds,
    string NotesUrl,
    IReadOnlyList<UpdateArtifact> Artifacts);

public sealed record UpdateManifestSignature(
    string KeyId,
    string Algorithm,
    string Signature);

public sealed record SignedUpdateManifest(
    UpdateManifestBody Body,
    string BodyJson,
    IReadOnlyList<UpdateManifestSignature> Signatures);

public sealed record UpdateTrustKey(string KeyId, string Algorithm, string PublicKey);

public sealed record UpdateTrustRoot(
    int Version,
    int Threshold,
    IReadOnlyList<UpdateTrustKey> Keys);

public sealed record VerifiedUpdate(SignedUpdateManifest Manifest, string ManifestHash);

public static class ThresholdUpdateVerifier
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    public static VerifiedUpdate Verify(
        SignedUpdateManifest manifest,
        UpdateTrustRoot trustRoot,
        DateTimeOffset? now = null)
    {
        ValidateTrustRoot(trustRoot);
        DateTimeOffset current = now ?? DateTimeOffset.UtcNow;
        string canonical = JsonSerializer.Serialize(manifest.Body, JsonOptions);
        if (manifest.Body.Version != 2
            || manifest.Body.Sequence < 1
            || string.IsNullOrWhiteSpace(manifest.Body.Channel)
            || manifest.Body.Channel.Length > 32
            || string.IsNullOrWhiteSpace(manifest.Body.ReleaseVersion)
            || manifest.Body.ReleaseVersion.Length > 64
            || manifest.Body.PublishedAtUnixMilliseconds > current.AddHours(24).ToUnixTimeMilliseconds()
            || manifest.Body.ExpiresAtUnixMilliseconds <= current.ToUnixTimeMilliseconds()
            || manifest.Body.ExpiresAtUnixMilliseconds
               > DateTimeOffset.FromUnixTimeMilliseconds(manifest.Body.PublishedAtUnixMilliseconds)
                   .AddDays(180).ToUnixTimeMilliseconds()
            || manifest.BodyJson != canonical
            || manifest.Body.Artifacts.Count is < 1 or > 16
            || manifest.Signatures.Count is < 1 or > 32)
        {
            throw new CryptographicException("Update manifest structure or lifetime is invalid");
        }
        foreach (UpdateArtifact artifact in manifest.Body.Artifacts)
        {
            if (string.IsNullOrWhiteSpace(artifact.Platform)
                || string.IsNullOrWhiteSpace(artifact.Architecture)
                || artifact.Size < 1
                || artifact.Sha256.Length != 64
                || !artifact.Sha256.All(Uri.IsHexDigit)
                || !Uri.TryCreate(artifact.Url, UriKind.RelativeOrAbsolute, out Uri? uri)
                || (uri.IsAbsoluteUri
                    ? uri.Scheme != Uri.UriSchemeHttps && !uri.IsLoopback
                    : !artifact.Url.StartsWith("/updates/", StringComparison.Ordinal)))
                throw new CryptographicException("Update artifact metadata is invalid");
        }

        byte[] bodyBytes = Encoding.UTF8.GetBytes(manifest.BodyJson);
        Dictionary<string, UpdateTrustKey> trusted = trustRoot.Keys.ToDictionary(value => value.KeyId, StringComparer.Ordinal);
        int valid = 0;
        foreach (UpdateManifestSignature signature in manifest.Signatures.DistinctBy(value => value.KeyId))
        {
            if (!trusted.TryGetValue(signature.KeyId, out UpdateTrustKey? key)
                || signature.Algorithm != "Ed25519"
                || key.Algorithm != "Ed25519") continue;
            try
            {
                var publicKey = (Ed25519PublicKeyParameters)PublicKeyFactory.CreateKey(
                    Convert.FromBase64String(key.PublicKey));
                var verifier = new Ed25519Signer();
                verifier.Init(false, publicKey);
                verifier.BlockUpdate(bodyBytes, 0, bodyBytes.Length);
                if (verifier.VerifySignature(Convert.FromBase64String(signature.Signature))) valid++;
            }
            catch
            {
                // Invalid individual signatures do not count toward the threshold.
            }
        }
        if (valid < trustRoot.Threshold)
            throw new CryptographicException("Update signature threshold was not reached");
        return new VerifiedUpdate(
            manifest,
            Convert.ToHexString(SHA256.HashData(bodyBytes)).ToLowerInvariant());
    }

    public static void VerifyArtifact(UpdateArtifact artifact, ReadOnlySpan<byte> content)
    {
        if (content.Length != artifact.Size
            || !CryptographicOperations.FixedTimeEquals(
                SHA256.HashData(content),
                Convert.FromHexString(artifact.Sha256)))
            throw new CryptographicException("Downloaded update does not match the signed artifact digest");
    }

    public static string KeyId(string subjectPublicKeyInfo)
    {
        byte[] digest = SHA256.HashData(Convert.FromBase64String(subjectPublicKeyInfo));
        return "upd1-" + Convert.ToHexString(digest.AsSpan(0, 12)).ToLowerInvariant();
    }

    private static void ValidateTrustRoot(UpdateTrustRoot root)
    {
        if (root.Version != 1
            || root.Keys.Count is < 1 or > 16
            || root.Threshold < 1
            || root.Threshold > root.Keys.Count
            || root.Keys.Select(value => value.KeyId).Distinct(StringComparer.Ordinal).Count() != root.Keys.Count)
            throw new CryptographicException("Update trust root is invalid");
        foreach (UpdateTrustKey key in root.Keys)
        {
            if (key.Algorithm != "Ed25519" || key.KeyId != KeyId(key.PublicKey))
                throw new CryptographicException("Update trust key is invalid");
            _ = (Ed25519PublicKeyParameters)PublicKeyFactory.CreateKey(Convert.FromBase64String(key.PublicKey));
        }
    }
}

public sealed class ThresholdUpdateClient
{
    private const string Purpose = "TuratText.UpdateState.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;
    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public ThresholdUpdateClient(HttpClient http, IProtectedStorage storage)
    {
        _http = http;
        _storage = storage;
    }

    public async Task<VerifiedUpdate> FetchAndPinAsync(
        Uri manifestUri,
        UpdateTrustRoot trustRoot,
        CancellationToken cancellationToken = default)
    {
        SignedUpdateManifest manifest = await _http.GetFromJsonAsync<SignedUpdateManifest>(
            manifestUri,
            JsonOptions,
            cancellationToken) ?? throw new CryptographicException("Update manifest is empty");
        VerifiedUpdate verified = ThresholdUpdateVerifier.Verify(manifest, trustRoot);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            Dictionary<string, PinnedUpdate> pins = await LoadAsync(cancellationToken);
            if (pins.TryGetValue(manifest.Body.Channel, out PinnedUpdate? pinned))
            {
                if (manifest.Body.Sequence < pinned.Sequence)
                    throw new CryptographicException("Update rollback was rejected");
                if (manifest.Body.Sequence == pinned.Sequence && verified.ManifestHash != pinned.ManifestHash)
                    throw new CryptographicException("Conflicting update manifest was rejected");
            }
            pins[manifest.Body.Channel] = new PinnedUpdate(manifest.Body.Sequence, verified.ManifestHash);
            await SaveAsync(pins, cancellationToken);
            return verified;
        }
        finally
        {
            _gate.Release();
        }
    }

    private async Task<Dictionary<string, PinnedUpdate>> LoadAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(StatePath)) return new Dictionary<string, PinnedUpdate>(StringComparer.Ordinal);
        byte[] encrypted = await File.ReadAllBytesAsync(StatePath, cancellationToken);
        return JsonSerializer.Deserialize<Dictionary<string, PinnedUpdate>>(
                   _storage.Unprotect(encrypted, Purpose),
                   JsonOptions) ?? new Dictionary<string, PinnedUpdate>(StringComparer.Ordinal);
    }

    private async Task SaveAsync(Dictionary<string, PinnedUpdate> values, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(StatePath)!);
        byte[] encrypted = _storage.Protect(JsonSerializer.SerializeToUtf8Bytes(values, JsonOptions), Purpose);
        string temporary = StatePath + ".new";
        await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
        File.Move(temporary, StatePath, true);
    }

    private string StatePath => Path.Combine(_storage.AppDirectory, "local-first", "update-state-v2.secure");
    private sealed record PinnedUpdate(long Sequence, string ManifestHash);
}
