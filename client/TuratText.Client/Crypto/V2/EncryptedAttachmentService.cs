using System.Buffers.Binary;
using System.Net.Http.Json;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.Transport.V2;

namespace TuratText.Client.Crypto.V2;

public sealed record AttachmentBlobReference(
    string NodeId,
    string BaseUrl,
    string ObjectId,
    string ReadCapability,
    DateTimeOffset ExpiresAt);

public sealed record EncryptedAttachmentManifest(
    int Version,
    string AttachmentId,
    string FileName,
    string MimeType,
    long PlaintextSize,
    int ChunkSize,
    int ChunkCount,
    string FileKey,
    string PlaintextSha256,
    IReadOnlyList<string> CiphertextChunkSha256,
    IReadOnlyList<AttachmentBlobReference> Blobs);

public sealed class EncryptedAttachmentService
{
    private const int DefaultChunkSize = 512 * 1024;
    private const int NonceSize = 12;
    private const int TagSize = 16;
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly HttpClient _http;

    public EncryptedAttachmentService(HttpClient http)
    {
        _http = http;
    }

    public async Task<EncryptedAttachmentManifest> EncryptAndUploadAsync(
        ReadOnlyMemory<byte> plaintext,
        string fileName,
        string mimeType,
        IReadOnlyCollection<NodeDescriptor> nodes,
        CancellationToken cancellationToken = default)
    {
        if (plaintext.Length == 0) throw new ArgumentException("Attachment is empty", nameof(plaintext));
        if (nodes.Count == 0) throw new InvalidOperationException("No Blob Nodes are available");
        string attachmentId = "att1-" + RandomToken(18);
        byte[] fileKey = RandomNumberGenerator.GetBytes(32);
        var encryptedChunks = new List<byte[]>();
        var digests = new List<string>();
        for (int offset = 0, index = 0; offset < plaintext.Length; offset += DefaultChunkSize, index++)
        {
            byte[] chunk = plaintext.Slice(offset, Math.Min(DefaultChunkSize, plaintext.Length - offset)).ToArray();
            byte[] nonce = RandomNumberGenerator.GetBytes(NonceSize);
            byte[] cipher = new byte[chunk.Length];
            byte[] tag = new byte[TagSize];
            using (var aes = new AesGcm(fileKey, TagSize))
            {
                aes.Encrypt(nonce, chunk, cipher, tag, ChunkAad(attachmentId, index));
            }
            byte[] packed = new byte[NonceSize + cipher.Length + TagSize];
            nonce.CopyTo(packed, 0);
            cipher.CopyTo(packed, NonceSize);
            tag.CopyTo(packed, NonceSize + cipher.Length);
            encryptedChunks.Add(packed);
            digests.Add(Convert.ToHexString(SHA256.HashData(packed)).ToLowerInvariant());
        }

        long ciphertextSize = encryptedChunks.Sum(chunk => (long)chunk.Length);
        var references = new List<AttachmentBlobReference>();
        foreach (NodeDescriptor node in nodes.Where(value => NodeDescriptorVerifier.Verify(value)).DistinctBy(value => value.NodeId))
        {
            string objectId = "blob1-" + RandomToken(18);
            string readCapability = RandomToken(32);
            string writeCapability = RandomToken(32);
            DateTimeOffset expiresAt = DateTimeOffset.UtcNow.AddDays(14);
            using HttpResponseMessage registration = await _http.PostAsJsonAsync(
                BuildUri(node.BaseUrl, "v2/blobs"),
                new
                {
                    objectId,
                    readCapability,
                    writeCapability,
                    expectedSize = ciphertextSize,
                    chunkSize = DefaultChunkSize + NonceSize + TagSize,
                    expiresAt
                },
                JsonOptions,
                cancellationToken);
            await EnsureSuccessAsync(registration, cancellationToken);
            for (int index = 0; index < encryptedChunks.Count; index++)
            {
                using var request = new HttpRequestMessage(
                    HttpMethod.Put,
                    BuildUri(node.BaseUrl, $"v2/blobs/{objectId}/chunks/{index}"));
                request.Headers.Add("X-Blob-Write-Capability", writeCapability);
                request.Headers.Add("X-Chunk-SHA256", digests[index]);
                request.Content = new ByteArrayContent(encryptedChunks[index]);
                request.Content.Headers.ContentType = new System.Net.Http.Headers.MediaTypeHeaderValue(
                    "application/octet-stream");
                using HttpResponseMessage upload = await _http.SendAsync(request, cancellationToken);
                await EnsureSuccessAsync(upload, cancellationToken);
            }
            references.Add(new AttachmentBlobReference(
                node.NodeId,
                node.BaseUrl,
                objectId,
                readCapability,
                expiresAt));
        }
        if (references.Count == 0) throw new InvalidOperationException("No trusted Blob Node accepted the attachment");
        return new EncryptedAttachmentManifest(
            2,
            attachmentId,
            Path.GetFileName(fileName),
            mimeType,
            plaintext.Length,
            DefaultChunkSize,
            encryptedChunks.Count,
            Convert.ToBase64String(fileKey),
            Convert.ToHexString(SHA256.HashData(plaintext.Span)).ToLowerInvariant(),
            digests,
            references);
    }

    public async Task<byte[]> DownloadAndDecryptAsync(
        EncryptedAttachmentManifest manifest,
        CancellationToken cancellationToken = default)
    {
        if (manifest.Version != 2 || manifest.ChunkCount < 1
            || manifest.CiphertextChunkSha256.Count != manifest.ChunkCount)
        {
            throw new CryptographicException("Attachment manifest is invalid");
        }
        Exception? lastError = null;
        foreach (AttachmentBlobReference blob in manifest.Blobs)
        {
            try
            {
                byte[] fileKey = Convert.FromBase64String(manifest.FileKey);
                using var output = new MemoryStream(checked((int)manifest.PlaintextSize));
                for (int index = 0; index < manifest.ChunkCount; index++)
                {
                    using var request = new HttpRequestMessage(
                        HttpMethod.Get,
                        BuildUri(blob.BaseUrl, $"v2/blobs/{blob.ObjectId}/chunks/{index}"));
                    request.Headers.Add("X-Blob-Read-Capability", blob.ReadCapability);
                    using HttpResponseMessage response = await _http.SendAsync(request, cancellationToken);
                    await EnsureSuccessAsync(response, cancellationToken);
                    byte[] packed = await response.Content.ReadAsByteArrayAsync(cancellationToken);
                    string digest = Convert.ToHexString(SHA256.HashData(packed)).ToLowerInvariant();
                    if (digest != manifest.CiphertextChunkSha256[index])
                    {
                        throw new CryptographicException("Attachment chunk digest mismatch");
                    }
                    int cipherLength = packed.Length - NonceSize - TagSize;
                    if (cipherLength < 0) throw new CryptographicException("Attachment chunk is truncated");
                    byte[] plaintext = new byte[cipherLength];
                    using (var aes = new AesGcm(fileKey, TagSize))
                    {
                        aes.Decrypt(
                            packed.AsSpan(0, NonceSize),
                            packed.AsSpan(NonceSize, cipherLength),
                            packed.AsSpan(NonceSize + cipherLength),
                            plaintext,
                            ChunkAad(manifest.AttachmentId, index));
                    }
                    await output.WriteAsync(plaintext, cancellationToken);
                }
                byte[] result = output.ToArray();
                if (result.LongLength != manifest.PlaintextSize
                    || Convert.ToHexString(SHA256.HashData(result)).ToLowerInvariant() != manifest.PlaintextSha256)
                {
                    throw new CryptographicException("Attachment plaintext verification failed");
                }
                CryptographicOperations.ZeroMemory(fileKey);
                return result;
            }
            catch (Exception exception) when (exception is not OperationCanceledException)
            {
                lastError = exception;
            }
        }
        throw new InvalidOperationException("No attachment replica could be downloaded", lastError);
    }

    private static byte[] ChunkAad(string attachmentId, int index)
    {
        byte[] id = Encoding.UTF8.GetBytes(attachmentId);
        byte[] result = new byte[id.Length + 4];
        id.CopyTo(result, 0);
        BinaryPrimitives.WriteInt32BigEndian(result.AsSpan(id.Length), index);
        return result;
    }

    private static Uri BuildUri(string baseUrl, string path) => new(new Uri(baseUrl.TrimEnd('/') + "/"), path);

    private static async Task EnsureSuccessAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        if (response.IsSuccessStatusCode) return;
        string details = await response.Content.ReadAsStringAsync(cancellationToken);
        throw new HttpRequestException($"Blob node request failed with HTTP {(int)response.StatusCode}: {details}");
    }

    private static string RandomToken(int bytes) =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
            .TrimEnd('=').Replace('+', '-').Replace('/', '_');
}
