using System.Security.Cryptography;
using System.Text.Json;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed class OwnedMailboxStore
{
    private const string StoragePurpose = "TuratText.OwnedMailboxes.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public OwnedMailboxStore(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public async Task<IReadOnlyList<OwnedMailboxRoute>> LoadAsync(CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            if (!File.Exists(Path)) return [];
            byte[] protectedBytes = await File.ReadAllBytesAsync(Path, cancellationToken);
            IReadOnlyList<OwnedMailboxRoute> routes = JsonSerializer.Deserialize<IReadOnlyList<OwnedMailboxRoute>>(
                                                         _storage.Unprotect(protectedBytes, StoragePurpose),
                                                         JsonOptions)
                                                     ?? throw new CryptographicException("Mailbox route store is damaged");
            return routes.Where(route =>
                    route.ExpiresAt > DateTimeOffset.UtcNow
                    && NodeDescriptorVerifier.Verify(route.Node))
                .ToList();
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task SaveAsync(
        IReadOnlyCollection<OwnedMailboxRoute> routes,
        CancellationToken cancellationToken = default)
    {
        if (routes.Any(route => !NodeDescriptorVerifier.Verify(route.Node)))
        {
            throw new CryptographicException("Cannot save an invalid node descriptor");
        }
        await _gate.WaitAsync(cancellationToken);
        try
        {
            Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
            byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(routes, JsonOptions);
            byte[] protectedBytes = _storage.Protect(plaintext, StoragePurpose);
            string temporary = Path + ".new";
            await File.WriteAllBytesAsync(temporary, protectedBytes, cancellationToken);
            File.Move(temporary, Path, overwrite: true);
        }
        finally
        {
            _gate.Release();
        }
    }

    private string Path => System.IO.Path.Combine(
        _storage.AppDirectory,
        "local-first",
        "owned-mailboxes-v2.secure");
}
