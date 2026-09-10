using System.Security.Cryptography;
using System.Text.Json;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed class OwnRoutingDescriptorStore
{
    private const string Purpose = "TuratText.OwnRoutingDescriptor.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;

    public OwnRoutingDescriptorStore(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public async Task SaveAsync(SignedRoutingDescriptor descriptor, CancellationToken cancellationToken = default)
    {
        if (!RoutingDescriptorService.Verify(descriptor))
            throw new CryptographicException("Own routing descriptor is invalid");
        Directory.CreateDirectory(Path.GetDirectoryName(StorePath)!);
        byte[] encrypted = _storage.Protect(
            JsonSerializer.SerializeToUtf8Bytes(descriptor, JsonOptions),
            Purpose);
        string temporary = StorePath + ".new";
        await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
        File.Move(temporary, StorePath, true);
    }

    public async Task<SignedRoutingDescriptor> LoadAsync(CancellationToken cancellationToken = default)
    {
        if (!File.Exists(StorePath)) throw new InvalidOperationException("Own routing descriptor is not published");
        byte[] encrypted = await File.ReadAllBytesAsync(StorePath, cancellationToken);
        SignedRoutingDescriptor descriptor = JsonSerializer.Deserialize<SignedRoutingDescriptor>(
                                                 _storage.Unprotect(encrypted, Purpose),
                                                 JsonOptions)
                                             ?? throw new CryptographicException("Own routing descriptor is damaged");
        return RoutingDescriptorService.Verify(descriptor)
            ? descriptor
            : throw new CryptographicException("Own routing descriptor signature is invalid");
    }

    private string StorePath => Path.Combine(_storage.AppDirectory, "local-first", "own-routing-v2.secure");
}
