using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Org.BouncyCastle.Crypto.Generators;
using Org.BouncyCastle.Crypto.Parameters;
using Org.BouncyCastle.Security;
using TuratText.Client.LocalFirst;
using TuratText.Client.Services;

namespace TuratText.Client.Crypto.V2;

public sealed class PrekeyStateService
{
    private const string StoragePurpose = "TuratText.PrekeyState.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private StoredPrekeyState? _state;

    public PrekeyStateService(IProtectedStorage storage, ProtocolIdentityService identity)
    {
        _storage = storage;
        _identity = identity;
    }

    public async Task<PrekeyPublication> GetOrCreatePublicationAsync(
        int desiredOneTimePrekeys = 50,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadOrCreateStateAsync(cancellationToken);
            bool changed = false;
            if (_state!.ExpiresAtUnixMilliseconds < DateTimeOffset.UtcNow.AddDays(7).ToUnixTimeMilliseconds())
            {
                _state = CreateState(checked(_state.Sequence + 1), desiredOneTimePrekeys);
                changed = true;
            }
            while (_state.OneTimePrekeys.Count < desiredOneTimePrekeys)
            {
                _state.OneTimePrekeys.Add(CreateOneTimePrekey());
                changed = true;
            }
            // ECDSA signatures are intentionally randomized, so every newly materialized
            // publication gets its own monotonically increasing sequence number.
            _state = _state with { Sequence = checked(_state.Sequence + 1) };
            changed = true;
            if (changed) await SaveAsync(cancellationToken);
            return BuildPublication(_state, _state.OneTimePrekeys.Select(value => value.PrekeyId));
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<PrekeyPublication> GetOrCreatePublicationForNodeAsync(
        string nodeId,
        int desiredOneTimePrekeys = 50,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(nodeId)) throw new ArgumentException("Node ID is required", nameof(nodeId));
        await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadOrCreateStateAsync(cancellationToken);
            bool changed = false;
            if (_state!.ExpiresAtUnixMilliseconds < DateTimeOffset.UtcNow.AddDays(7).ToUnixTimeMilliseconds())
            {
                _state = CreateState(checked(_state.Sequence + 1), desiredOneTimePrekeys);
                changed = true;
            }
            if (_state.NodeAssignments is null)
            {
                _state = _state with
                {
                    NodeAssignments = new Dictionary<string, List<string>>(StringComparer.Ordinal)
                };
            }
            if (!_state.NodeAssignments.TryGetValue(nodeId, out List<string>? assigned))
            {
                assigned = [];
                _state.NodeAssignments[nodeId] = assigned;
                changed = true;
            }
            var existingIds = _state.OneTimePrekeys.Select(value => value.PrekeyId).ToHashSet(StringComparer.Ordinal);
            changed |= assigned.RemoveAll(value => !existingIds.Contains(value)) > 0;
            var reserved = _state.NodeAssignments
                .Where(pair => pair.Key != nodeId)
                .SelectMany(pair => pair.Value)
                .ToHashSet(StringComparer.Ordinal);
            int targetCount = Math.Clamp(desiredOneTimePrekeys, 1, 200);
            while (assigned.Count < targetCount)
            {
                StoredOneTimePrekey? available = _state.OneTimePrekeys.FirstOrDefault(value =>
                    !reserved.Contains(value.PrekeyId) && !assigned.Contains(value.PrekeyId, StringComparer.Ordinal));
                if (available is null)
                {
                    available = CreateOneTimePrekey();
                    _state.OneTimePrekeys.Add(available);
                }
                assigned.Add(available.PrekeyId);
                changed = true;
            }
            _state = _state with { Sequence = checked(_state.Sequence + 1) };
            changed = true;
            if (changed) await SaveAsync(cancellationToken);
            return BuildPublication(_state, assigned);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<LocalPrekeySecrets> GetSecretsAsync(CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadOrCreateStateAsync(cancellationToken);
            return new LocalPrekeySecrets(
                _state!.IdentityDhPrivateKey,
                _state.SignedPrekeyId,
                _state.SignedPrekeyPrivateKey,
                _state.PqPrekeyId,
                _state.PqPrekeyPrivateKey);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<string?> ConsumeOneTimePrivateKeyAsync(
        string? prekeyId,
        CancellationToken cancellationToken = default)
    {
        if (prekeyId is null) return null;
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadOrCreateStateAsync(cancellationToken);
            StoredOneTimePrekey? value = _state!.OneTimePrekeys.FirstOrDefault(
                key => string.Equals(key.PrekeyId, prekeyId, StringComparison.Ordinal));
            if (value is null) return null;
            _state.OneTimePrekeys.Remove(value);
            if (_state.NodeAssignments is not null)
            {
                foreach (List<string> assigned in _state.NodeAssignments.Values)
                {
                    assigned.Remove(prekeyId);
                }
            }
            await SaveAsync(cancellationToken);
            return value.PrivateKey;
        }
        finally
        {
            _gate.Release();
        }
    }

    public static bool VerifyPublication(PrekeyPublication publication)
    {
        if (!ProtocolIdentityService.VerifyDeviceCertificate(publication.Identity)
            || publication.Identity.UserId != publication.SignedPrekey.Descriptor.UserId
            || publication.Identity.DeviceId != publication.SignedPrekey.Descriptor.DeviceId)
        {
            return false;
        }
        if (!string.Equals(
                publication.SignedPrekey.DescriptorJson,
                JsonSerializer.Serialize(publication.SignedPrekey.Descriptor, JsonOptions),
                StringComparison.Ordinal)
            || !ProtocolIdentityService.VerifyDeviceData(
                publication.Identity,
                Encoding.UTF8.GetBytes(publication.SignedPrekey.DescriptorJson),
                publication.SignedPrekey.Signature))
        {
            return false;
        }
        return publication.OneTimePrekeys.All(prekey =>
            ProtocolIdentityService.VerifyDeviceData(
                publication.Identity,
                OneTimeSigningBytes(prekey.PrekeyId, prekey.PublicKey),
                prekey.Signature));
    }

    private async Task LoadOrCreateStateAsync(CancellationToken cancellationToken)
    {
        if (_state is not null) return;
        string path = StatePath;
        if (File.Exists(path))
        {
            byte[] encrypted = await File.ReadAllBytesAsync(path, cancellationToken);
            _state = JsonSerializer.Deserialize<StoredPrekeyState>(
                         _storage.Unprotect(encrypted, StoragePurpose),
                         JsonOptions)
                     ?? throw new CryptographicException("Prekey state is damaged");
            return;
        }
        _state = CreateState(1, 50);
        await SaveAsync(cancellationToken);
    }

    private StoredPrekeyState CreateState(long sequence, int oneTimeCount)
    {
        var random = new SecureRandom();
        var identityDh = new X25519PrivateKeyParameters(random);
        var signedPrekey = new X25519PrivateKeyParameters(random);
        var pqGenerator = new MLKemKeyPairGenerator();
        pqGenerator.Init(new MLKemKeyGenerationParameters(random, MLKemParameters.ml_kem_768));
        var pqPair = pqGenerator.GenerateKeyPair();
        var pqPrivate = (MLKemPrivateKeyParameters)pqPair.Private;
        var oneTime = new List<StoredOneTimePrekey>(oneTimeCount);
        for (int index = 0; index < oneTimeCount; index++) oneTime.Add(CreateOneTimePrekey());
        return new StoredPrekeyState(
            sequence,
            Convert.ToBase64String(identityDh.GetEncoded()),
            "spk1-" + RandomToken(12),
            Convert.ToBase64String(signedPrekey.GetEncoded()),
            "pqk1-" + RandomToken(12),
            Convert.ToBase64String(pqPrivate.GetEncoded()),
            DateTimeOffset.UtcNow.AddDays(30).ToUnixTimeMilliseconds(),
            oneTime,
            new Dictionary<string, List<string>>(StringComparer.Ordinal));
    }

    private PrekeyPublication BuildPublication(StoredPrekeyState state, IEnumerable<string> selectedPrekeyIds)
    {
        ProtocolIdentity identity = _identity.Current!;
        var identityDh = new X25519PrivateKeyParameters(Convert.FromBase64String(state.IdentityDhPrivateKey));
        var signedPrekey = new X25519PrivateKeyParameters(Convert.FromBase64String(state.SignedPrekeyPrivateKey));
        var pqPrivate = MLKemPrivateKeyParameters.FromEncoding(
            MLKemParameters.ml_kem_768,
            Convert.FromBase64String(state.PqPrekeyPrivateKey));
        var descriptor = new DevicePrekeyDescriptor(
            2,
            identity.UserId,
            identity.DeviceId,
            Convert.ToBase64String(identityDh.GeneratePublicKey().GetEncoded()),
            state.SignedPrekeyId,
            Convert.ToBase64String(signedPrekey.GeneratePublicKey().GetEncoded()),
            state.PqPrekeyId,
            Convert.ToBase64String(pqPrivate.GetPublicKeyEncoded()),
            state.Sequence,
            state.ExpiresAtUnixMilliseconds);
        string descriptorJson = JsonSerializer.Serialize(descriptor, JsonOptions);
        var signed = new SignedDevicePrekey(
            descriptor,
            descriptorJson,
            _identity.SignDeviceData(Encoding.UTF8.GetBytes(descriptorJson)));
        HashSet<string> selected = selectedPrekeyIds.ToHashSet(StringComparer.Ordinal);
        IReadOnlyList<OneTimePrekeyPublic> oneTime = state.OneTimePrekeys
            .Where(value => selected.Contains(value.PrekeyId))
            .Select(value =>
            {
                var key = new X25519PrivateKeyParameters(Convert.FromBase64String(value.PrivateKey));
                string publicKey = Convert.ToBase64String(key.GeneratePublicKey().GetEncoded());
                return new OneTimePrekeyPublic(
                    value.PrekeyId,
                    publicKey,
                    _identity.SignDeviceData(OneTimeSigningBytes(value.PrekeyId, publicKey)));
            }).ToList();
        return new PrekeyPublication(identity, signed, oneTime);
    }

    private StoredOneTimePrekey CreateOneTimePrekey()
    {
        var key = new X25519PrivateKeyParameters(new SecureRandom());
        return new StoredOneTimePrekey(
            "otk1-" + RandomToken(12),
            Convert.ToBase64String(key.GetEncoded()));
    }

    private async Task SaveAsync(CancellationToken cancellationToken)
    {
        string path = StatePath;
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(_state, JsonOptions);
        byte[] encrypted = _storage.Protect(plaintext, StoragePurpose);
        string temporary = path + ".new";
        await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
        File.Move(temporary, path, overwrite: true);
    }

    private string StatePath => Path.Combine(_storage.AppDirectory, "local-first", "prekeys-v2.secure");

    private static byte[] OneTimeSigningBytes(string prekeyId, string publicKey) =>
        Encoding.UTF8.GetBytes("TuratText.OneTimePrekey.v2\n" + prekeyId + "\n" + publicKey);

    private static string RandomToken(int bytes) =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
            .TrimEnd('=').Replace('+', '-').Replace('/', '_');

    public sealed record LocalPrekeySecrets(
        string IdentityDhPrivateKey,
        string SignedPrekeyId,
        string SignedPrekeyPrivateKey,
        string PqPrekeyId,
        string PqPrekeyPrivateKey);

    private sealed record StoredPrekeyState(
        long Sequence,
        string IdentityDhPrivateKey,
        string SignedPrekeyId,
        string SignedPrekeyPrivateKey,
        string PqPrekeyId,
        string PqPrekeyPrivateKey,
        long ExpiresAtUnixMilliseconds,
        List<StoredOneTimePrekey> OneTimePrekeys,
        Dictionary<string, List<string>>? NodeAssignments);

    private sealed record StoredOneTimePrekey(string PrekeyId, string PrivateKey);
}
