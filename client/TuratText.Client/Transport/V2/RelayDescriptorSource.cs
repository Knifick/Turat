using System.Text.Json;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed class RelayDescriptorSource
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web) { WriteIndented = true };
    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public RelayDescriptorSource(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public async Task<IReadOnlyList<RelayDescriptor>> LoadAsync(CancellationToken cancellationToken = default)
    {
        var descriptors = new List<RelayDescriptor>();
        if (File.Exists(Path))
        {
            descriptors.AddRange(JsonSerializer.Deserialize<IReadOnlyList<RelayDescriptor>>(
                                     await File.ReadAllTextAsync(Path, cancellationToken), JsonOptions) ?? []);
        }
        string? configured = Environment.GetEnvironmentVariable("TURATTEXT_V2_RELAY_DESCRIPTOR_FILES");
        if (!string.IsNullOrWhiteSpace(configured))
        {
            foreach (string file in configured.Split(';', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries))
            {
                if (!File.Exists(file)) continue;
                RelayDescriptor? value = JsonSerializer.Deserialize<RelayDescriptor>(
                    await File.ReadAllTextAsync(file, cancellationToken), JsonOptions);
                if (value is not null) descriptors.Add(value);
            }
        }
        return descriptors.Where(RelayDescriptorVerifier.Verify)
            .DistinctBy(value => value.RelayId)
            .ToList();
    }

    public async Task AddPinnedAsync(RelayDescriptor descriptor, CancellationToken cancellationToken = default)
    {
        if (!RelayDescriptorVerifier.Verify(descriptor))
            throw new System.Security.Cryptography.CryptographicException("Relay descriptor is invalid");
        await _gate.WaitAsync(cancellationToken);
        try
        {
            List<RelayDescriptor> values = (await LoadAsync(cancellationToken)).ToList();
            values.RemoveAll(value => value.RelayId == descriptor.RelayId);
            values.Add(descriptor);
            Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
            string temporary = Path + ".new";
            await File.WriteAllTextAsync(temporary, JsonSerializer.Serialize(values, JsonOptions), cancellationToken);
            File.Move(temporary, Path, overwrite: true);
        }
        finally
        {
            _gate.Release();
        }
    }

    private string Path => System.IO.Path.Combine(_storage.AppDirectory, "relay-descriptors-v2.json");
}

