using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;

namespace TuratText.Windows.Interop;

internal sealed partial class RustCore : IDisposable
{
    private nint _handle;

    static RustCore()
    {
        NativeLibrary.SetDllImportResolver(typeof(RustCore).Assembly, ResolveNativeLibrary);
    }

    public RustCore()
    {
        string appDirectory = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "TuratText");
        Directory.CreateDirectory(appDirectory);
        byte[] vaultKey = LoadOrCreateVaultKey(appDirectory);
        try
        {
            _handle = Native.Create(appDirectory, Convert.ToBase64String(vaultKey));
        }
        finally
        {
            CryptographicOperations.ZeroMemory(vaultKey);
        }
        if (_handle == 0) throw new InvalidOperationException("Rust core не удалось инициализировать");
    }

    public CoreResponse Invoke(object command)
    {
        ObjectDisposedException.ThrowIf(_handle == 0, this);
        string request = JsonSerializer.Serialize(command, JsonOptions.Default);
        nint pointer = Native.Invoke(_handle, request);
        if (pointer == 0) throw new InvalidOperationException("Rust core вернул пустой ответ");
        try
        {
            string response = Marshal.PtrToStringUTF8(pointer)
                              ?? throw new InvalidOperationException("Ответ Rust core не является UTF-8");
            return JsonSerializer.Deserialize<CoreResponse>(response, JsonOptions.Default)
                   ?? throw new InvalidOperationException("Rust core вернул некорректный JSON");
        }
        finally
        {
            Native.FreeString(pointer);
        }
    }

    public void Dispose()
    {
        if (_handle == 0) return;
        Native.Destroy(_handle);
        _handle = 0;
    }

    private static nint ResolveNativeLibrary(string libraryName, System.Reflection.Assembly assembly, DllImportSearchPath? searchPath)
    {
        if (!string.Equals(libraryName, "turattext_core.dll", StringComparison.OrdinalIgnoreCase))
        {
            return 0;
        }

        string[] candidates =
        [
            Path.Combine(AppContext.BaseDirectory, "native", "turattext_core.dll"),
            Path.Combine(AppContext.BaseDirectory, "turattext_core.dll")
        ];
        foreach (string candidate in candidates)
        {
            if (File.Exists(candidate) && NativeLibrary.TryLoad(candidate, out nint handle))
            {
                return handle;
            }
        }
        return 0;
    }

    private static byte[] LoadOrCreateVaultKey(string appDirectory)
    {
        string path = Path.Combine(appDirectory, "vault-key-v3.dpapi");
        if (File.Exists(path))
        {
            return ProtectedData.Unprotect(File.ReadAllBytes(path), "TuratText.LocalVault.v3"u8.ToArray(), DataProtectionScope.CurrentUser);
        }
        byte[] value = RandomNumberGenerator.GetBytes(32);
        byte[] encrypted = ProtectedData.Protect(value, "TuratText.LocalVault.v3"u8.ToArray(), DataProtectionScope.CurrentUser);
        string temporary = path + ".new";
        File.WriteAllBytes(temporary, encrypted);
        File.Move(temporary, path, true);
        return value;
    }

    private static partial class Native
    {
        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_core_create", StringMarshalling = StringMarshalling.Utf8)]
        internal static partial nint Create(string appDirectory, string vaultKeyBase64);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_core_invoke", StringMarshalling = StringMarshalling.Utf8)]
        internal static partial nint Invoke(nint handle, string requestJson);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_core_destroy")]
        internal static partial void Destroy(nint handle);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_string_free")]
        internal static partial void FreeString(nint value);
    }
}

internal static class JsonOptions
{
    public static readonly JsonSerializerOptions Default = new(JsonSerializerDefaults.Web)
    {
        PropertyNameCaseInsensitive = true
    };
}
