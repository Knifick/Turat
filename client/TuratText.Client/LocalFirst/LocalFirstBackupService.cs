using System.IO.Compression;
using System.Security.Cryptography;
using System.Text;
using TuratText.Client.Services;

namespace TuratText.Client.LocalFirst;

public sealed class LocalFirstBackupService
{
    private static readonly byte[] Magic = "TTBACKUP2"u8.ToArray();
    private const int Iterations = 600_000;
    private const int TagSize = 16;
    private readonly IProtectedStorage _storage;
    private readonly LocalEventStore _events;

    private static readonly BackupEntry[] ProtectedEntries =
    [
        new("local-first/protocol-identity-v2.secure", "identity.json", "TuratText.ProtocolIdentity.v2"),
        new("local-first/prekeys-v2.secure", "prekeys.json", "TuratText.PrekeyState.v2"),
        new("local-first/ratchet-sessions-v2.secure", "ratchet.json", "TuratText.RatchetSessions.v2"),
        new("local-first/owned-mailboxes-v2.secure", "owned-mailboxes.json", "TuratText.OwnedMailboxes.v2"),
        new("local-first/own-routing-v2.secure", "own-routing.json", "TuratText.OwnRoutingDescriptor.v2"),
        new("local-first/private-mailbox-grants-v2.secure", "private-mailbox-grants.json", "TuratText.PrivateMailboxGrants.v2"),
        new("local-first/contacts-v2.secure", "contacts.json", "TuratText.Contacts.v2"),
        new("local-first/group-epochs-v2.secure", "groups.json", "TuratText.GroupEpochs.v2"),
        new("local-first/device-list-v2.secure", "device-list.json", "TuratText.DeviceList.v2"),
        new("local-first/transparency-v2.secure", "transparency.json", "TuratText.Transparency.v2"),
        new("local-first/routing-sequence-v2.secure", "routing-sequence.txt", "TuratText.RoutingSequence.v2"),
        new("local-first/username-sequence-v2.secure", "username-sequence.txt", "TuratText.UsernameSequence.v2"),
        new("local-first/update-state-v2.secure", "update-state.json", "TuratText.UpdateState.v2"),
        new("local-first/metadata-protection-v2.secure", "metadata-protection.txt", "TuratText.MetadataProtection.v2"),
        new("local-first/event-vault-key-v1.secure", "event-vault-key.bin", "TuratText.LocalEventVault.v1")
    ];

    private static readonly RawEntry[] RawEntries =
    [
        new("bootstrap-nodes-v2.json", "bootstrap-nodes.json"),
        new("relay-descriptors-v2.json", "relay-descriptors.json")
    ];

    public LocalFirstBackupService(IProtectedStorage storage, LocalEventStore events)
    {
        _storage = storage;
        _events = events;
    }

    public async Task CreateAsync(
        string destinationPath,
        string passphrase,
        CancellationToken cancellationToken = default)
    {
        ValidatePassphrase(passphrase);
        string temporaryDirectory = Path.Combine(Path.GetTempPath(), "TuratTextBackup-" + Guid.NewGuid().ToString("N"));
        Directory.CreateDirectory(temporaryDirectory);
        try
        {
            string databaseSnapshot = Path.Combine(temporaryDirectory, "events-v2.db");
            await _events.CreateDatabaseSnapshotAsync(databaseSnapshot, cancellationToken);
            byte[] archive;
            using (var output = new MemoryStream())
            {
                using (var zip = new ZipArchive(output, ZipArchiveMode.Create, leaveOpen: true))
                {
                    await AddBytesAsync(zip, "manifest.txt", Encoding.UTF8.GetBytes(
                        "TuratText local-first backup v2\ncreated=" + DateTimeOffset.UtcNow.ToString("O")), cancellationToken);
                    foreach (BackupEntry entry in ProtectedEntries)
                    {
                        string source = Path.Combine(_storage.AppDirectory, entry.Source.Replace('/', Path.DirectorySeparatorChar));
                        if (!File.Exists(source)) continue;
                        byte[] plaintext = _storage.Unprotect(await File.ReadAllBytesAsync(source, cancellationToken), entry.Purpose);
                        await AddBytesAsync(zip, entry.ArchiveName, plaintext, cancellationToken);
                        CryptographicOperations.ZeroMemory(plaintext);
                    }
                    await AddFileAsync(zip, "events-v2.db", databaseSnapshot, cancellationToken);
                    foreach (RawEntry entry in RawEntries)
                    {
                        string source = Path.Combine(_storage.AppDirectory, entry.Source);
                        if (File.Exists(source)) await AddFileAsync(zip, entry.ArchiveName, source, cancellationToken);
                    }
                }
                archive = output.ToArray();
            }
            byte[] salt = RandomNumberGenerator.GetBytes(16);
            byte[] nonce = RandomNumberGenerator.GetBytes(12);
            byte[] key = Rfc2898DeriveBytes.Pbkdf2(passphrase, salt, Iterations, HashAlgorithmName.SHA256, 32);
            byte[] ciphertext = new byte[archive.Length];
            byte[] tag = new byte[TagSize];
            using (var aes = new AesGcm(key, TagSize))
                aes.Encrypt(nonce, archive, ciphertext, tag, Magic);
            Directory.CreateDirectory(Path.GetDirectoryName(Path.GetFullPath(destinationPath))!);
            string temporary = destinationPath + ".new";
            await using (FileStream stream = File.Create(temporary))
            {
                await stream.WriteAsync(Magic, cancellationToken);
                await stream.WriteAsync(salt, cancellationToken);
                await stream.WriteAsync(nonce, cancellationToken);
                await stream.WriteAsync(tag, cancellationToken);
                await stream.WriteAsync(ciphertext, cancellationToken);
            }
            File.Move(temporary, destinationPath, overwrite: true);
            CryptographicOperations.ZeroMemory(key);
            CryptographicOperations.ZeroMemory(archive);
        }
        finally
        {
            Directory.Delete(temporaryDirectory, recursive: true);
        }
    }

    public async Task RestoreAsync(
        string sourcePath,
        string passphrase,
        CancellationToken cancellationToken = default)
    {
        ValidatePassphrase(passphrase);
        byte[] container = await File.ReadAllBytesAsync(sourcePath, cancellationToken);
        int headerSize = Magic.Length + 16 + 12 + TagSize;
        if (container.Length <= headerSize || !container.AsSpan(0, Magic.Length).SequenceEqual(Magic))
            throw new CryptographicException("Backup format is invalid");
        byte[] salt = container.AsSpan(Magic.Length, 16).ToArray();
        byte[] nonce = container.AsSpan(Magic.Length + 16, 12).ToArray();
        byte[] tag = container.AsSpan(Magic.Length + 28, TagSize).ToArray();
        byte[] ciphertext = container.AsSpan(headerSize).ToArray();
        byte[] key = Rfc2898DeriveBytes.Pbkdf2(passphrase, salt, Iterations, HashAlgorithmName.SHA256, 32);
        byte[] archive = new byte[ciphertext.Length];
        using (var aes = new AesGcm(key, TagSize)) aes.Decrypt(nonce, ciphertext, tag, archive, Magic);
        using var input = new MemoryStream(archive, writable: false);
        using var zip = new ZipArchive(input, ZipArchiveMode.Read);
        if (zip.GetEntry("manifest.txt") is null || zip.GetEntry("events-v2.db") is null)
            throw new CryptographicException("Backup is incomplete");
        foreach (BackupEntry entry in ProtectedEntries)
        {
            ZipArchiveEntry? archived = zip.GetEntry(entry.ArchiveName);
            if (archived is null) continue;
            byte[] plaintext = await ReadEntryAsync(archived, cancellationToken);
            string destination = Path.Combine(_storage.AppDirectory, entry.Source.Replace('/', Path.DirectorySeparatorChar));
            await WriteAtomicallyAsync(destination, _storage.Protect(plaintext, entry.Purpose), cancellationToken);
            CryptographicOperations.ZeroMemory(plaintext);
        }
        await RestoreRawAsync(zip.GetEntry("events-v2.db")!, "local-first/events-v2.db", cancellationToken);
        foreach (RawEntry entry in RawEntries)
        {
            ZipArchiveEntry? archived = zip.GetEntry(entry.ArchiveName);
            if (archived is not null) await RestoreRawAsync(archived, entry.Source, cancellationToken);
        }
        CryptographicOperations.ZeroMemory(key);
        CryptographicOperations.ZeroMemory(archive);
    }

    private async Task RestoreRawAsync(ZipArchiveEntry entry, string relativePath, CancellationToken cancellationToken)
    {
        byte[] value = await ReadEntryAsync(entry, cancellationToken);
        await WriteAtomicallyAsync(Path.Combine(_storage.AppDirectory, relativePath.Replace('/', Path.DirectorySeparatorChar)), value, cancellationToken);
    }

    private static async Task AddFileAsync(ZipArchive zip, string name, string path, CancellationToken cancellationToken) =>
        await AddBytesAsync(zip, name, await File.ReadAllBytesAsync(path, cancellationToken), cancellationToken);

    private static async Task AddBytesAsync(ZipArchive zip, string name, byte[] value, CancellationToken cancellationToken)
    {
        ZipArchiveEntry entry = zip.CreateEntry(name, CompressionLevel.SmallestSize);
        await using Stream stream = entry.Open();
        await stream.WriteAsync(value, cancellationToken);
    }

    private static async Task<byte[]> ReadEntryAsync(ZipArchiveEntry entry, CancellationToken cancellationToken)
    {
        if (entry.Length > 1024L * 1024 * 1024) throw new CryptographicException("Backup entry is too large");
        await using Stream stream = entry.Open();
        using var output = new MemoryStream(checked((int)entry.Length));
        await stream.CopyToAsync(output, cancellationToken);
        return output.ToArray();
    }

    private static async Task WriteAtomicallyAsync(string destination, byte[] value, CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
        string temporary = destination + ".restore-new";
        await File.WriteAllBytesAsync(temporary, value, cancellationToken);
        File.Move(temporary, destination, overwrite: true);
    }

    private static void ValidatePassphrase(string passphrase)
    {
        if (string.IsNullOrWhiteSpace(passphrase) || passphrase.Length < 10)
            throw new ArgumentException("Backup passphrase must contain at least 10 characters", nameof(passphrase));
    }

    private sealed record BackupEntry(string Source, string ArchiveName, string Purpose);
    private sealed record RawEntry(string Source, string ArchiveName);
}
