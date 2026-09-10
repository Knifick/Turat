using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Org.BouncyCastle.Crypto.Parameters;
using Org.BouncyCastle.Crypto.Signers;
using Org.BouncyCastle.Security;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed record TransparencyOperation(
    long Sequence,
    string OperationId,
    string OperationType,
    string SubjectId,
    string PayloadHash,
    string Payload,
    string Signature,
    DateTimeOffset CreatedAt);

public sealed record TransparencyCheckpoint(
    long TreeSize,
    string RootHash,
    long CreatedAtUnixMilliseconds,
    string Signature,
    string NodeId);

public sealed record TransparencySyncResult(
    string NodeId,
    long TreeSize,
    string RootHash,
    int NewOperations);

public sealed class TransparencyLogClient
{
    private const string StoragePurpose = "TuratText.Transparency.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;
    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public TransparencyLogClient(HttpClient http, IProtectedStorage storage)
    {
        _http = http;
        _storage = storage;
    }

    public async Task<TransparencySyncResult> SyncAsync(
        NodeDescriptor node,
        CancellationToken cancellationToken = default)
    {
        if (!NodeDescriptorVerifier.Verify(node))
            throw new CryptographicException("Cannot sync transparency from an invalid node");
        await _gate.WaitAsync(cancellationToken);
        try
        {
            Dictionary<string, StoredTransparencyState> all = await LoadAsync(cancellationToken);
            if (!all.TryGetValue(node.NodeId, out StoredTransparencyState? state))
                state = new StoredTransparencyState([], null);
            int added = await FetchOperationsAsync(node, state, cancellationToken);
            TransparencyCheckpoint checkpoint = await GetCheckpointAsync(node, cancellationToken);
            if (checkpoint.TreeSize > state.PayloadHashes.Count)
            {
                added += await FetchOperationsAsync(node, state, cancellationToken);
                checkpoint = await GetCheckpointAsync(node, cancellationToken);
            }
            ValidateCheckpoint(node, state, checkpoint);
            all[node.NodeId] = state with { Checkpoint = checkpoint };
            await SaveAsync(all, cancellationToken);
            return new TransparencySyncResult(node.NodeId, checkpoint.TreeSize, checkpoint.RootHash, added);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task ObservePeerCheckpointAsync(
        TransparencyCheckpoint peer,
        CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            Dictionary<string, StoredTransparencyState> all = await LoadAsync(cancellationToken);
            if (all.TryGetValue(peer.NodeId, out StoredTransparencyState? state)
                && state.Checkpoint is { } local
                && local.TreeSize == peer.TreeSize
                && !string.Equals(local.RootHash, peer.RootHash, StringComparison.Ordinal))
            {
                throw new CryptographicException(
                    $"Transparency equivocation detected for node {peer.NodeId} at tree size {peer.TreeSize}");
            }
        }
        finally
        {
            _gate.Release();
        }
    }

    private async Task<int> FetchOperationsAsync(
        NodeDescriptor node,
        StoredTransparencyState state,
        CancellationToken cancellationToken)
    {
        int added = 0;
        while (true)
        {
            long after = state.PayloadHashes.Count;
            IReadOnlyList<TransparencyOperation> operations =
                await _http.GetFromJsonAsync<IReadOnlyList<TransparencyOperation>>(
                    BuildUri(node.BaseUrl, $"v2/transparency/operations?after={after}&limit=1000"),
                    JsonOptions,
                    cancellationToken) ?? throw new InvalidOperationException("Transparency response is empty");
            foreach (TransparencyOperation operation in operations)
            {
                long expected = state.PayloadHashes.Count + 1L;
                if (operation.Sequence != expected
                    || !string.Equals(
                        operation.PayloadHash,
                        Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(operation.Payload))).ToLowerInvariant(),
                        StringComparison.Ordinal)
                    || !VerifyNodeSignature(node, OperationSigningBytes(operation), operation.Signature))
                {
                    throw new CryptographicException("Transparency operation chain is invalid");
                }
                state.PayloadHashes.Add(operation.PayloadHash);
                added++;
            }
            if (operations.Count < 1000) return added;
        }
    }

    private async Task<TransparencyCheckpoint> GetCheckpointAsync(
        NodeDescriptor node,
        CancellationToken cancellationToken)
    {
        TransparencyCheckpoint checkpoint = await _http.GetFromJsonAsync<TransparencyCheckpoint>(
                                                BuildUri(node.BaseUrl, "v2/transparency/checkpoint"),
                                                JsonOptions,
                                                cancellationToken)
                                            ?? throw new InvalidOperationException("Transparency checkpoint is empty");
        if (checkpoint.NodeId != node.NodeId
            || checkpoint.TreeSize < 0
            || !VerifyNodeSignature(node, CheckpointSigningBytes(checkpoint), checkpoint.Signature))
        {
            throw new CryptographicException("Transparency checkpoint signature is invalid");
        }
        return checkpoint;
    }

    private static void ValidateCheckpoint(
        NodeDescriptor node,
        StoredTransparencyState state,
        TransparencyCheckpoint checkpoint)
    {
        if (checkpoint.TreeSize != state.PayloadHashes.Count)
            throw new CryptographicException("Transparency checkpoint does not cover the downloaded log");
        if (state.Checkpoint is { } previous
            && (checkpoint.TreeSize < previous.TreeSize
                || (checkpoint.TreeSize == previous.TreeSize
                    && checkpoint.RootHash != previous.RootHash)))
        {
            throw new CryptographicException($"Transparency rollback or equivocation detected for {node.NodeId}");
        }
        string calculated = MerkleRoot(state.PayloadHashes);
        if (!string.Equals(calculated, checkpoint.RootHash, StringComparison.Ordinal))
            throw new CryptographicException("Transparency Merkle root mismatch");
    }

    private static bool VerifyNodeSignature(NodeDescriptor node, byte[] value, string signature)
    {
        try
        {
            var publicKey = (Ed25519PublicKeyParameters)PublicKeyFactory.CreateKey(
                Convert.FromBase64String(node.PublicKey));
            var verifier = new Ed25519Signer();
            verifier.Init(false, publicKey);
            verifier.BlockUpdate(value, 0, value.Length);
            return verifier.VerifySignature(Convert.FromBase64String(signature));
        }
        catch
        {
            return false;
        }
    }

    private static byte[] OperationSigningBytes(TransparencyOperation value) => Encoding.UTF8.GetBytes(
        value.OperationId + "\n" + value.OperationType + "\n" + value.SubjectId + "\n" + value.PayloadHash);

    private static byte[] CheckpointSigningBytes(TransparencyCheckpoint value) => Encoding.UTF8.GetBytes(
        "TuratText.TransparencyCheckpoint\n" + value.TreeSize + "\n" + value.RootHash + "\n"
        + value.CreatedAtUnixMilliseconds);

    private static string MerkleRoot(IReadOnlyList<string> encodedHashes)
    {
        if (encodedHashes.Count == 0)
            return Convert.ToHexString(SHA256.HashData([])).ToLowerInvariant();
        List<byte[]> level = encodedHashes.Select(Convert.FromHexString).ToList();
        while (level.Count > 1)
        {
            var next = new List<byte[]>((level.Count + 1) / 2);
            for (int index = 0; index < level.Count; index += 2)
            {
                byte[] left = level[index];
                byte[] right = level[Math.Min(index + 1, level.Count - 1)];
                next.Add(SHA256.HashData([1, .. left, .. right]));
            }
            level = next;
        }
        return Convert.ToHexString(level[0]).ToLowerInvariant();
    }

    private async Task<Dictionary<string, StoredTransparencyState>> LoadAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(Path)) return new Dictionary<string, StoredTransparencyState>(StringComparer.Ordinal);
        byte[] encrypted = await File.ReadAllBytesAsync(Path, cancellationToken);
        return JsonSerializer.Deserialize<Dictionary<string, StoredTransparencyState>>(
                   _storage.Unprotect(encrypted, StoragePurpose),
                   JsonOptions)
               ?? throw new CryptographicException("Transparency state is damaged");
    }

    private async Task SaveAsync(
        Dictionary<string, StoredTransparencyState> value,
        CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
        byte[] protectedBytes = _storage.Protect(
            JsonSerializer.SerializeToUtf8Bytes(value, JsonOptions),
            StoragePurpose);
        string temporary = Path + ".new";
        await File.WriteAllBytesAsync(temporary, protectedBytes, cancellationToken);
        File.Move(temporary, Path, overwrite: true);
    }

    private string Path => System.IO.Path.Combine(_storage.AppDirectory, "local-first", "transparency-v2.secure");
    private static Uri BuildUri(string baseUrl, string path) => new(new Uri(baseUrl.TrimEnd('/') + "/"), path);

    private sealed record StoredTransparencyState(
        List<string> PayloadHashes,
        TransparencyCheckpoint? Checkpoint);
}
