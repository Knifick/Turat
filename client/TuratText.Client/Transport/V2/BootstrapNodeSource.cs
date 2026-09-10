using System.Text.Json;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public sealed class BootstrapNodeSource
{
    public const string DefaultBootstrapUrl = "https://turattext.rplacefree.store";
    public const string DefaultNodeId = "ttn1-c6fcc7ef3f4693c7359d5ce47bd413bb516429bec7b118ca04b8da2fd3254a97";

    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web)
    {
        WriteIndented = true
    };
    private readonly IProtectedStorage _storage;

    public BootstrapNodeSource(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public async Task<IReadOnlyList<(Uri Uri, string? ExpectedNodeId)>> LoadAsync(
        CancellationToken cancellationToken = default)
    {
        var values = new List<BootstrapEntry>();
        if (!string.Equals(
                Environment.GetEnvironmentVariable("TURATTEXT_V2_DISABLE_DEFAULT_BOOTSTRAP"),
                "1",
                StringComparison.Ordinal))
        {
            values.Add(new BootstrapEntry(DefaultBootstrapUrl, DefaultNodeId));
        }
        string? configured = Environment.GetEnvironmentVariable("TURATTEXT_V2_BOOTSTRAP_URLS");
        if (!string.IsNullOrWhiteSpace(configured))
        {
            values.AddRange(configured.Split(
                    [';', ','],
                    StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                .Select(url => new BootstrapEntry(url, null)));
        }
        if (File.Exists(Path))
        {
            IReadOnlyList<BootstrapEntry>? stored = JsonSerializer.Deserialize<IReadOnlyList<BootstrapEntry>>(
                await File.ReadAllTextAsync(Path, cancellationToken),
                JsonOptions);
            if (stored is not null) values.AddRange(stored);
        }
        return values
            .Where(value => Uri.TryCreate(value.Url, UriKind.Absolute, out _))
            .Select(value => (Uri: new Uri(value.Url), value.NodeId))
            .GroupBy(value => value.Uri.GetComponents(UriComponents.HttpRequestUrl, UriFormat.Unescaped),
                StringComparer.OrdinalIgnoreCase)
            .Select(group => group.OrderByDescending(value => value.NodeId is not null).First())
            .ToList();
    }

    public async Task AddPinnedAsync(
        NodeDescriptor descriptor,
        CancellationToken cancellationToken = default)
    {
        if (!NodeDescriptorVerifier.Verify(descriptor))
        {
            throw new System.Security.Cryptography.CryptographicException("Cannot pin an invalid node descriptor");
        }
        List<BootstrapEntry> entries = (await LoadFileAsync(cancellationToken)).ToList();
        entries.RemoveAll(value => value.NodeId == descriptor.NodeId || value.Url == descriptor.BaseUrl);
        entries.Add(new BootstrapEntry(descriptor.BaseUrl, descriptor.NodeId));
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
        string temporary = Path + ".new";
        await File.WriteAllTextAsync(temporary, JsonSerializer.Serialize(entries, JsonOptions), cancellationToken);
        File.Move(temporary, Path, overwrite: true);
    }

    private async Task<IReadOnlyList<BootstrapEntry>> LoadFileAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(Path)) return [];
        return JsonSerializer.Deserialize<IReadOnlyList<BootstrapEntry>>(
                   await File.ReadAllTextAsync(Path, cancellationToken),
                   JsonOptions)
               ?? [];
    }

    private string Path => System.IO.Path.Combine(_storage.AppDirectory, "bootstrap-nodes-v2.json");

    private sealed record BootstrapEntry(string Url, string? NodeId);
}
