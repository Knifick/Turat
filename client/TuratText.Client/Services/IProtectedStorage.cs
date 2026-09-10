namespace TuratText.Client.Services;

public interface IProtectedStorage
{
    string AppDirectory { get; }
    byte[] Protect(byte[] plaintext, string purpose);
    byte[] Unprotect(byte[] ciphertext, string purpose);
}
