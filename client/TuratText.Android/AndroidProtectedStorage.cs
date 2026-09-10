using System.Text;
using System.Security.Cryptography;
using Android.Content;
using Android.Security.Keystore;
using Java.Security;
using Javax.Crypto;
using Javax.Crypto.Spec;
using TuratText.Client.Services;

namespace TuratText.Android;

public sealed class AndroidProtectedStorage : IProtectedStorage
{
    private const string KeyAlias = "turattext.local.storage.v1";
    private const string KeyProvider = "AndroidKeyStore";
    private readonly Context _context;

    public AndroidProtectedStorage(Context context)
    {
        _context = context.ApplicationContext ?? context;
    }

    public string AppDirectory => _context.FilesDir?.AbsolutePath
        ?? throw new InvalidOperationException("Android application directory is unavailable");

    public byte[] Protect(byte[] plaintext, string purpose)
    {
        Cipher cipher = Cipher.GetInstance("AES/GCM/NoPadding")
            ?? throw new InvalidOperationException("AES-GCM is unavailable");
        cipher.Init(Javax.Crypto.CipherMode.EncryptMode, GetOrCreateKey());
        cipher.UpdateAAD(Encoding.UTF8.GetBytes(purpose));
        byte[] nonce = cipher.GetIV() ?? throw new InvalidOperationException("AES-GCM nonce was not generated");
        byte[] encrypted = cipher.DoFinal(plaintext)
            ?? throw new InvalidOperationException("Android Keystore encryption failed");

        byte[] packed = new byte[2 + nonce.Length + encrypted.Length];
        packed[0] = 1;
        packed[1] = checked((byte) nonce.Length);
        Buffer.BlockCopy(nonce, 0, packed, 2, nonce.Length);
        Buffer.BlockCopy(encrypted, 0, packed, 2 + nonce.Length, encrypted.Length);
        return packed;
    }

    public byte[] Unprotect(byte[] ciphertext, string purpose)
    {
        if (ciphertext.Length < 15 || ciphertext[0] != 1)
        {
            throw new CryptographicException("Unsupported protected storage format");
        }

        int nonceLength = ciphertext[1];
        if (nonceLength < 12 || 2 + nonceLength >= ciphertext.Length)
        {
            throw new CryptographicException("Protected storage payload is damaged");
        }

        byte[] nonce = ciphertext.AsSpan(2, nonceLength).ToArray();
        byte[] encrypted = ciphertext.AsSpan(2 + nonceLength).ToArray();
        Cipher cipher = Cipher.GetInstance("AES/GCM/NoPadding")
            ?? throw new InvalidOperationException("AES-GCM is unavailable");
        cipher.Init(Javax.Crypto.CipherMode.DecryptMode, GetOrCreateKey(), new GCMParameterSpec(128, nonce));
        cipher.UpdateAAD(Encoding.UTF8.GetBytes(purpose));
        return cipher.DoFinal(encrypted)
            ?? throw new CryptographicException("Android Keystore decryption failed");
    }

    private static IKey GetOrCreateKey()
    {
        KeyStore keyStore = KeyStore.GetInstance(KeyProvider)
            ?? throw new InvalidOperationException("Android Keystore is unavailable");
        keyStore.Load(null);
        if (!keyStore.ContainsAlias(KeyAlias))
        {
            KeyGenerator generator = KeyGenerator.GetInstance(KeyProperties.KeyAlgorithmAes, KeyProvider)
                ?? throw new InvalidOperationException("Android Keystore AES generator is unavailable");
            var spec = new KeyGenParameterSpec.Builder(
                    KeyAlias,
                    KeyStorePurpose.Encrypt | KeyStorePurpose.Decrypt)
                .SetBlockModes(KeyProperties.BlockModeGcm)
                .SetEncryptionPaddings(KeyProperties.EncryptionPaddingNone)
                .SetRandomizedEncryptionRequired(true)
                .Build();
            generator.Init(spec);
            generator.GenerateKey();
        }

        return keyStore.GetKey(KeyAlias, null)
            ?? throw new InvalidOperationException("Android Keystore key is unavailable");
    }
}
