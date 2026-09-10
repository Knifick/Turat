using System.Security.Cryptography;
using System.Text;

#pragma warning disable CA1416

namespace TuratText.Client.Services;

public sealed class WindowsProtectedStorage : IProtectedStorage
{
    public string AppDirectory => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "TuratText");

    public byte[] Protect(byte[] plaintext, string purpose)
    {
        return ProtectedData.Protect(
            plaintext,
            Encoding.UTF8.GetBytes(purpose),
            DataProtectionScope.CurrentUser);
    }

    public byte[] Unprotect(byte[] ciphertext, string purpose)
    {
        return ProtectedData.Unprotect(
            ciphertext,
            Encoding.UTF8.GetBytes(purpose),
            DataProtectionScope.CurrentUser);
    }
}

#pragma warning restore CA1416
