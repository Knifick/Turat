using System.Security.Cryptography;
using System.Text;
using TuratText.Client.Services;

namespace TuratText.Client.Transport.V2;

public enum MetadataProtectionMode
{
    Fast,
    Balanced,
    HighPrivacy
}

public sealed class MetadataProtectionSettings
{
    private const string Purpose = "TuratText.MetadataProtection.v2";
    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _gate = new(1, 1);

    public MetadataProtectionSettings(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public async Task<MetadataProtectionMode> GetAsync(CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            if (!File.Exists(PathValue)) return MetadataProtectionMode.Fast;
            byte[] protectedValue = await File.ReadAllBytesAsync(PathValue, cancellationToken);
            string value = Encoding.ASCII.GetString(_storage.Unprotect(protectedValue, Purpose));
            return Enum.TryParse(value, ignoreCase: false, out MetadataProtectionMode mode)
                ? mode
                : MetadataProtectionMode.Fast;
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task SetAsync(MetadataProtectionMode mode, CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            Directory.CreateDirectory(System.IO.Path.GetDirectoryName(PathValue)!);
            byte[] encrypted = _storage.Protect(Encoding.ASCII.GetBytes(mode.ToString()), Purpose);
            string temporary = PathValue + ".new";
            await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
            File.Move(temporary, PathValue, true);
        }
        finally
        {
            _gate.Release();
        }
    }

    public static TimeSpan RandomSendDelay(MetadataProtectionMode mode)
    {
        (int minimum, int maximum) = mode switch
        {
            MetadataProtectionMode.Balanced => (250, 2_000),
            MetadataProtectionMode.HighPrivacy => (3_000, 15_000),
            _ => (0, 0)
        };
        return maximum == 0
            ? TimeSpan.Zero
            : TimeSpan.FromMilliseconds(RandomNumberGenerator.GetInt32(minimum, maximum + 1));
    }

    private string PathValue => System.IO.Path.Combine(
        _storage.AppDirectory,
        "local-first",
        "metadata-protection-v2.secure");
}
