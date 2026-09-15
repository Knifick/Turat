using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text.Json;

namespace TuratText.Windows.Interop;

internal sealed partial class RustCore : IDisposable
{
    private nint _handle;

    /// <summary>
    /// Время жизни хэндла. Ожидание конверта держит указатель десятки секунд в фоновом потоке,
    /// поэтому освобождение ядра откладывается до конца последнего такого ожидания.
    /// </summary>
    private readonly object _lifetime = new();
    private int _waiting;
    private bool _disposed;

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
        ObjectDisposedException.ThrowIf(_disposed || _handle == 0, this);
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

    /// <summary>
    /// Открывает вложение для потокового чтения. Ядро при этом не блокируется: у читателя
    /// своя блокировка, поэтому воспроизведение видео не ждёт фоновую синхронизацию.
    /// </summary>
    public nint OpenMedia(string path)
    {
        ObjectDisposedException.ThrowIf(_handle == 0, this);
        return Native.MediaOpen(_handle, path);
    }

    public static long MediaLength(nint media) => media == 0 ? -1 : Native.MediaLength(media);

    /// <summary>Читает отрезок вложения; возвращает прочитанное количество байт или -1.</summary>
    public static unsafe int ReadMedia(nint media, long offset, byte[] buffer, int bufferOffset, int count)
    {
        if (media == 0) return -1;
        ArgumentOutOfRangeException.ThrowIfNegative(bufferOffset);
        ArgumentOutOfRangeException.ThrowIfGreaterThan(bufferOffset + count, buffer.Length);
        fixed (byte* pointer = buffer)
        {
            long read = Native.MediaRead(media, (ulong)offset, pointer + bufferOffset, (nuint)count);
            return read < 0 ? -1 : (int)read;
        }
    }

    public static void CloseMedia(nint media)
    {
        if (media != 0) Native.MediaClose(media);
    }

    /// <summary>
    /// Ждёт входящий конверт на Node до <paramref name="seconds"/> секунд: 1 — сообщение
    /// пришло, 0 — окно истекло, -1 — ждать негде или связь оборвалась. Ядро при этом не
    /// блокируется, поэтому отправка сообщения не ждёт конца окна.
    /// </summary>
    public int WaitForEnvelopes(int seconds)
    {
        nint handle;
        lock (_lifetime)
        {
            if (_disposed || _handle == 0) return -1;
            _waiting++;
            handle = _handle;
        }
        try
        {
            return Native.WaitForEnvelopes(handle, seconds);
        }
        finally
        {
            lock (_lifetime)
            {
                _waiting--;
                ReleaseIfIdle();
            }
        }
    }

    public void Dispose()
    {
        lock (_lifetime)
        {
            _disposed = true;
            ReleaseIfIdle();
        }
    }

    /// <summary>Вызывать только под <see cref="_lifetime"/>.</summary>
    private void ReleaseIfIdle()
    {
        if (!_disposed || _waiting != 0 || _handle == 0) return;
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

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_core_wait_for_envelopes")]
        internal static partial int WaitForEnvelopes(nint handle, int seconds);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_core_destroy")]
        internal static partial void Destroy(nint handle);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_string_free")]
        internal static partial void FreeString(nint value);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_media_open", StringMarshalling = StringMarshalling.Utf8)]
        internal static partial nint MediaOpen(nint handle, string path);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_media_length")]
        internal static partial long MediaLength(nint media);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_media_read")]
        internal static unsafe partial long MediaRead(nint media, ulong offset, byte* buffer, nuint length);

        [LibraryImport("turattext_core.dll", EntryPoint = "turattext_media_close")]
        internal static partial void MediaClose(nint media);
    }
}

internal static class JsonOptions
{
    public static readonly JsonSerializerOptions Default = new(JsonSerializerDefaults.Web)
    {
        PropertyNameCaseInsensitive = true
    };
}
