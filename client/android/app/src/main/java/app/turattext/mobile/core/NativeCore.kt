package app.turattext.mobile.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

object NativeCore {
    init { System.loadLibrary("turattext_core") }

    private var handle: Long = 0

    @Synchronized
    fun initialize(context: Context) {
        if (handle != 0L) return
        val vaultKey = VaultKey.loadOrCreate(context.applicationContext)
        handle = nativeCreate(context.filesDir.absolutePath, Base64.encodeToString(vaultKey, Base64.NO_WRAP))
        require(handle != 0L) { "Rust core не удалось инициализировать" }
        vaultKey.fill(0)
    }

    @Synchronized
    fun invoke(request: String): String {
        check(handle != 0L) { "Rust core не инициализирован" }
        return nativeInvoke(handle, request)
    }

    @Synchronized
    fun close() {
        if (handle != 0L) nativeDestroy(handle)
        handle = 0
    }

    private external fun nativeCreate(appDir: String, vaultKeyBase64: String): Long
    private external fun nativeInvoke(handle: Long, request: String): String
    private external fun nativeDestroy(handle: Long)
}

private object VaultKey {
    private const val Alias = "TuratText.LocalVault.v3"
    private const val Preference = "turattext-secure"
    private const val WrappedKey = "wrapped-vault-key-v3"

    fun loadOrCreate(context: Context): ByteArray {
        val wrappingKey = getOrCreateWrappingKey()
        val preferences = context.getSharedPreferences(Preference, Context.MODE_PRIVATE)
        val stored = preferences.getString(WrappedKey, null)
        if (stored != null) {
            val packed = Base64.decode(stored, Base64.NO_WRAP)
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, wrappingKey, GCMParameterSpec(128, packed, 0, 12))
            return cipher.doFinal(packed, 12, packed.size - 12)
        }
        val raw = ByteArray(32).also(java.security.SecureRandom()::nextBytes)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, wrappingKey)
        val packed = cipher.iv + cipher.doFinal(raw)
        preferences.edit().putString(WrappedKey, Base64.encodeToString(packed, Base64.NO_WRAP)).commit()
        return raw
    }

    private fun getOrCreateWrappingKey(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(Alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(Alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return generator.generateKey()
    }
}

