using System.Security.Cryptography;
using System.Text.Json;
using System.Text.Json.Serialization;
using Avalonia.Media;
using TuratText.Client.Services;
using TuratText.Client.Transport.V2;
using TuratText.Client.Ui;

namespace TuratText.Client.Messaging.V2;

public sealed record LocalContact(
    string UserId,
    string DisplayName,
    SignedRoutingDescriptor Routing,
    DateTimeOffset AddedAt,
    bool FingerprintVerified,
    bool PendingApproval = false)
{
    public string DisplayLabel => PendingApproval ? DisplayName + " · запрос" : DisplayName;
    public string Initials => string.IsNullOrWhiteSpace(DisplayName)
        ? "T"
        : DisplayName.Trim()[..1].ToUpperInvariant();
    public string ConversationStatus => PendingApproval ? "Запрос на общение" : "Защищённый чат";

    /// <summary>Stable per-contact gradient so avatars stay recognisable between sessions.</summary>
    [JsonIgnore]
    public IBrush AvatarBrush => AvatarPalette.For(UserId);

    [JsonIgnore]
    public string ShortUserId => UserId.Length <= 18 ? UserId : UserId[..10] + "…" + UserId[^6..];

    [JsonIgnore]
    public string SecurityBadge => FingerprintVerified ? "Fingerprint сверен" : "Fingerprint не сверен";
}

public sealed class LocalContactStore
{
    private const string StoragePurpose = "TuratText.Contacts.v2";
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public LocalContactStore(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public async Task<IReadOnlyList<LocalContact>> LoadAsync(CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            if (!File.Exists(Path)) return [];
            byte[] encrypted = await File.ReadAllBytesAsync(Path, cancellationToken);
            IReadOnlyList<LocalContact> contacts = JsonSerializer.Deserialize<IReadOnlyList<LocalContact>>(
                                                       _storage.Unprotect(encrypted, StoragePurpose),
                                                       JsonOptions)
                                                   ?? throw new CryptographicException("Contact store is damaged");
            return contacts.Where(contact => RoutingDescriptorService.Verify(contact.Routing)).ToList();
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task SaveAsync(LocalContact contact, CancellationToken cancellationToken = default)
    {
        if (contact.UserId != contact.Routing.Descriptor.UserId
            || !RoutingDescriptorService.Verify(contact.Routing))
        {
            throw new CryptographicException("Contact routing descriptor is invalid");
        }
        await _gate.WaitAsync(cancellationToken);
        try
        {
            List<LocalContact> contacts = await LoadUnlockedAsync(cancellationToken);
            LocalContact? existing = contacts.FirstOrDefault(value => value.UserId == contact.UserId);
            if (existing is not null
                && existing.Routing.Descriptor.Sequence > contact.Routing.Descriptor.Sequence)
            {
                throw new CryptographicException("Contact routing descriptor rollback detected");
            }
            contacts.RemoveAll(value => value.UserId == contact.UserId);
            contacts.Add(contact);
            Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
            byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(contacts, JsonOptions);
            byte[] encrypted = _storage.Protect(plaintext, StoragePurpose);
            string temporary = Path + ".new";
            await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
            File.Move(temporary, Path, overwrite: true);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task SetVerifiedAsync(string userId, bool verified, CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            List<LocalContact> contacts = await LoadUnlockedAsync(cancellationToken);
            int index = contacts.FindIndex(contact => contact.UserId == userId);
            if (index < 0) return;
            contacts[index] = contacts[index] with { FingerprintVerified = verified };
            byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(contacts, JsonOptions);
            byte[] encrypted = _storage.Protect(plaintext, StoragePurpose);
            await File.WriteAllBytesAsync(Path, encrypted, cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task SetPendingApprovalAsync(
        string userId,
        bool pending,
        CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            List<LocalContact> contacts = await LoadUnlockedAsync(cancellationToken);
            int index = contacts.FindIndex(contact => contact.UserId == userId);
            if (index < 0) return;
            contacts[index] = contacts[index] with { PendingApproval = pending };
            await SaveUnlockedAsync(contacts, cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task RemoveAsync(string userId, CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            List<LocalContact> contacts = await LoadUnlockedAsync(cancellationToken);
            contacts.RemoveAll(contact => contact.UserId == userId);
            await SaveUnlockedAsync(contacts, cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
    }

    private async Task<List<LocalContact>> LoadUnlockedAsync(CancellationToken cancellationToken)
    {
        if (!File.Exists(Path)) return [];
        byte[] encrypted = await File.ReadAllBytesAsync(Path, cancellationToken);
        return JsonSerializer.Deserialize<List<LocalContact>>(
                   _storage.Unprotect(encrypted, StoragePurpose),
                   JsonOptions)
               ?? [];
    }

    private async Task SaveUnlockedAsync(List<LocalContact> contacts, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
        byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(contacts, JsonOptions);
        byte[] encrypted = _storage.Protect(plaintext, StoragePurpose);
        string temporary = Path + ".new";
        await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
        File.Move(temporary, Path, true);
    }

    private string Path => System.IO.Path.Combine(_storage.AppDirectory, "local-first", "contacts-v2.secure");
}
