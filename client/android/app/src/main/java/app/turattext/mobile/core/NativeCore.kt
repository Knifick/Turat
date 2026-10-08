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

    /**
     * Время жизни хэндла. Ожидание конверта держит указатель десятки секунд и живёт в фоновом
     * потоке, который не свернуть отменой корутины, — поэтому освобождение ядра откладывается
     * до конца последнего такого ожидания.
     */
    private val lifetime = Any()
    private var waiting = 0
    private var disposed = false

    @Synchronized
    fun initialize(context: Context) {
        synchronized(lifetime) {
            // Ядро могло пережить закрытие, если в этот момент кто-то ждал конверт.
            if (handle != 0L) {
                disposed = false
                return
            }
        }
        val vaultKey = VaultKey.loadOrCreate(context.applicationContext)
        val created = nativeCreate(context.filesDir.absolutePath, Base64.encodeToString(vaultKey, Base64.NO_WRAP))
        require(created != 0L) { "Rust core не удалось инициализировать" }
        vaultKey.fill(0)
        synchronized(lifetime) {
            handle = created
            disposed = false
        }
    }

    @Synchronized
    fun invoke(request: String): String {
        check(handle != 0L) { "Rust core не инициализирован" }
        return nativeInvoke(handle, request)
    }

    @Synchronized
    fun close() {
        synchronized(lifetime) {
            disposed = true
            releaseIfIdle()
        }
    }

    /** Вызывать только под [lifetime]. */
    private fun releaseIfIdle() {
        if (disposed && waiting == 0 && handle != 0L) {
            nativeDestroy(handle)
            handle = 0
        }
    }

    /**
     * Открывает вложение для потокового чтения.
     *
     * Намеренно не под `@Synchronized`: плеер читает файл параллельно с обычными командами,
     * и ждать очередной фоновой синхронизации ради следующего куска видео он не должен —
     * своя блокировка на каждое вложение есть внутри Rust.
     */
    fun openMedia(path: String): Long {
        val current = handle
        if (current == 0L) return 0
        return nativeMediaOpen(current, path)
    }

    fun mediaLength(media: Long): Long = if (media == 0L) -1 else nativeMediaLength(media)

    /** Читает до [length] байт с позиции [offset]; 0 — конец файла, -1 — ошибка. */
    fun readMedia(media: Long, offset: Long, buffer: ByteArray, length: Int): Int =
        if (media == 0L) -1 else nativeMediaRead(media, offset, buffer, length)

    fun closeMedia(media: Long) {
        if (media != 0L) nativeMediaClose(media)
    }

    /**
     * Ждёт входящий конверт на Node до [seconds] секунд: 1 — сообщение пришло, 0 — окно истекло,
     * -1 — ждать негде или связь оборвалась.
     *
     * Намеренно не под `@Synchronized`: ожидание длится десятки секунд, и всё это время ядро
     * обязано оставаться свободным для отправки и остальных действий пользователя.
     */
    fun waitForEnvelopes(seconds: Int): Int {
        val current = synchronized(lifetime) {
            if (disposed || handle == 0L) return -1
            waiting += 1
            handle
        }
        try {
            return nativeWaitForEnvelopes(current, seconds)
        } finally {
            synchronized(lifetime) {
                waiting -= 1
                releaseIfIdle()
            }
        }
    }

    /**
     * Звонок. Аудиопотоки отдают и забирают кадр каждые 20 мс, а экран спрашивает состояние
     * десятки раз в секунду, поэтому всё это идёт мимо `@Synchronized`: у звонка своя
     * блокировка внутри Rust, и ждать окончания синхронизации ради кадра голоса нельзя.
     * Хэндл на время вызова удерживается так же, как при ожидании конверта.
     */
    fun callPush(pcm: ShortArray) {
        borrow(Unit) { nativeCallPush(it, pcm, pcm.size) }
    }

    /** Кадр для динамика. `false` — звонка с медиапотоком нет, играть нечего. */
    fun callPull(output: ShortArray): Boolean = borrow(false) { nativeCallPull(it, output, output.size) }

    /** Состояние звонка в JSON; `{"active":false}` — звонка нет. */
    fun callStatus(): String = borrow("{\"active\":false}") { nativeCallStatus(it) }

    /** `mute`, `unmute`, `hangup` или `dismiss`. */
    fun callAction(name: String): Boolean = borrow(false) { nativeCallAction(it, name) }

    private inline fun <T> borrow(fallback: T, block: (Long) -> T): T {
        val current = synchronized(lifetime) {
            if (disposed || handle == 0L) return fallback
            waiting += 1
            handle
        }
        try {
            return block(current)
        } finally {
            synchronized(lifetime) {
                waiting -= 1
                releaseIfIdle()
            }
        }
    }

    private external fun nativeCreate(appDir: String, vaultKeyBase64: String): Long
    private external fun nativeInvoke(handle: Long, request: String): String
    private external fun nativeDestroy(handle: Long)
    private external fun nativeMediaOpen(handle: Long, path: String): Long
    private external fun nativeMediaLength(media: Long): Long
    private external fun nativeMediaRead(media: Long, offset: Long, buffer: ByteArray, length: Int): Int
    private external fun nativeMediaClose(media: Long)
    private external fun nativeWaitForEnvelopes(handle: Long, seconds: Int): Int
    private external fun nativeCallPush(handle: Long, pcm: ShortArray, length: Int)
    private external fun nativeCallPull(handle: Long, output: ShortArray, length: Int): Boolean
    private external fun nativeCallStatus(handle: Long): String
    private external fun nativeCallAction(handle: Long, name: String): Boolean
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

