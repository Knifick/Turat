package app.turattext.mobile.update

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.Uri
import android.os.Build
import android.provider.Settings
import app.turattext.mobile.BuildConfig
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/** Опубликованный релиз, который новее установленной версии. */
data class ReleaseInfo(
    val version: String,
    val tag: String,
    val title: String,
    val notes: String,
    val pageUrl: String,
    val downloadUrl: String,
    val size: Long,
    val sha256: String,
)

/** Всё, что интерфейсу нужно знать об обновлении. */
data class UpdateState(
    val currentVersion: String = AppUpdater.currentVersionLabel,
    val available: ReleaseInfo? = null,
    /** Строка над списком скрыта до следующего запуска. */
    val bannerDismissed: Boolean = false,
    /** Версия, от которой пользователь отказался. */
    val skippedVersion: String? = null,
    val checking: Boolean = false,
    /** Доля загруженного APK; `null` — загрузки нет. */
    val downloading: Float? = null,
    val message: String? = null,
) {
    val showBanner: Boolean
        get() = available != null && !bannerDismissed && available.version != skippedVersion
}

/**
 * Обновления из GitHub Releases официального репозитория.
 *
 * APK проверяется дважды: сначала SHA-256 с опубликованным GitHub хешем файла (или со строкой из
 * `SHA256SUMS.txt` того же релиза), затем самим Android — поверх установленного приложения ляжет
 * только APK, подписанный тем же release-ключом.
 */
object AppUpdater {
    const val Owner = "Knifick"
    const val Repository = "Turat"

    private const val ReleasesUrl = "https://api.github.com/repos/$Owner/$Repository/releases?per_page=15"
    private const val ApkAsset = "Turat.apk"
    private const val ChecksumsAsset = "SHA256SUMS.txt"
    private const val DownloadDirectory = "updates"

    private val currentVersion: List<Int> = parseVersion(BuildConfig.VERSION_NAME) ?: listOf(0, 0, 0, 0)
    val currentVersionLabel: String = label(currentVersion)

    private val _installFailures = MutableSharedFlow<String>(extraBufferCapacity = 1)

    /** Отказ системного установщика приходит в receiver, а показать его нужно на экране. */
    val installFailures = _installFailures.asSharedFlow()

    internal fun reportInstallFailure(message: String) {
        _installFailures.tryEmit(message)
    }

    /** Самый свежий стабильный релиз новее установленного или `null`. */
    suspend fun findUpdate(): ReleaseInfo? = withContext(Dispatchers.IO) {
        val releases = JSONArray(readText(ReleasesUrl))
        var best: Pair<List<Int>, ReleaseInfo>? = null
        for (index in 0 until releases.length()) {
            val release = releases.getJSONObject(index)
            if (release.optBoolean("draft") || release.optBoolean("prerelease")) continue
            val tag = release.optString("tag_name")
            val version = parseVersion(tag) ?: continue
            if (compare(version, currentVersion) <= 0) continue
            if (best != null && compare(version, best.first) <= 0) continue

            val assets = release.optJSONArray("assets") ?: continue
            val apk = findAsset(assets, ApkAsset) ?: continue
            val sha256 = digestOf(apk) ?: checksumFromList(assets, ApkAsset) ?: continue
            best = version to ReleaseInfo(
                version = label(version),
                tag = tag,
                title = release.optString("name").ifBlank { tag },
                notes = release.optString("body"),
                pageUrl = release.optString("html_url"),
                downloadUrl = apk.getString("browser_download_url"),
                size = apk.optLong("size"),
                sha256 = sha256,
            )
        }
        best?.second
    }

    /**
     * Загружает APK во внутренний кэш и сверяет SHA-256. Уже скачанный и проверенный файл
     * повторно не загружается: после выдачи разрешения на установку пользователь нажимает
     * «Обновить» ещё раз.
     */
    suspend fun download(context: Context, release: ReleaseInfo, onProgress: (Float) -> Unit): File =
        withContext(Dispatchers.IO) {
            val directory = File(context.cacheDir, DownloadDirectory).apply { mkdirs() }
            val target = File(directory, "Turat-${release.version}.apk")
            if (target.exists() && sha256Of(target).equals(release.sha256, ignoreCase = true)) {
                onProgress(1f)
                return@withContext target
            }

            val partial = File(directory, target.name + ".part")
            val connection = open(release.downloadUrl, "application/octet-stream")
            try {
                if (connection.responseCode !in 200..299) {
                    throw IOException("GitHub ответил ${connection.responseCode}")
                }
                val total = connection.contentLengthLong.takeIf { it > 0 } ?: release.size
                val digest = MessageDigest.getInstance("SHA-256")
                connection.inputStream.use { input ->
                    partial.outputStream().use { output ->
                        val buffer = ByteArray(1 shl 16)
                        var done = 0L
                        var reported = -1f
                        while (true) {
                            currentCoroutineContext().ensureActive()
                            val read = input.read(buffer)
                            if (read < 0) break
                            output.write(buffer, 0, read)
                            digest.update(buffer, 0, read)
                            done += read
                            val fraction = if (total > 0) (done.toFloat() / total).coerceAtMost(1f) else 0f
                            // Полпроцента на шаг: иначе интерфейс перерисовывался бы тысячи раз.
                            if (fraction - reported >= 0.005f) {
                                reported = fraction
                                onProgress(fraction)
                            }
                        }
                    }
                }
                val actual = digest.digest().toHex()
                if (!actual.equals(release.sha256, ignoreCase = true)) {
                    partial.delete()
                    throw IOException("Контрольная сумма загруженного файла не совпала с опубликованной")
                }
            } catch (error: Throwable) {
                partial.delete()
                throw error
            } finally {
                connection.disconnect()
            }
            if (!partial.renameTo(target)) throw IOException("Не удалось сохранить загруженный APK")
            target
        }

    /** Скачанные APK нужны только до установки: новая версия убирает их при запуске. */
    fun clearDownloads(context: Context) {
        File(context.cacheDir, DownloadDirectory).deleteRecursively()
    }

    /** С Android 8 приложению нужно отдельное разрешение пользователя на установку APK. */
    fun canInstallPackages(context: Context): Boolean =
        Build.VERSION.SDK_INT < Build.VERSION_CODES.O || context.packageManager.canRequestPackageInstalls()

    fun openInstallPermissionSettings(context: Context) {
        context.startActivity(
            Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${context.packageName}"))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }

    /**
     * Передаёт APK системному установщику. Подтверждение пользователь видит в системном окне:
     * его открывает [UpdateInstallReceiver], когда сессия переходит в ожидание действия.
     */
    fun install(context: Context, apk: File) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(context.packageName)
        }
        val sessionId = installer.createSession(params)
        try {
            installer.openSession(sessionId).use { session ->
                session.openWrite(ApkAsset, 0, apk.length()).use { output ->
                    apk.inputStream().use { it.copyTo(output) }
                    session.fsync(output)
                }
                // Установщик дописывает в intent статус сессии, поэтому на Android 12+ он mutable.
                val flags = PendingIntent.FLAG_UPDATE_CURRENT or
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0
                val status = PendingIntent.getBroadcast(
                    context,
                    sessionId,
                    Intent(context, UpdateInstallReceiver::class.java),
                    flags,
                )
                session.commit(status.intentSender)
            }
        } catch (error: Throwable) {
            installer.abandonSession(sessionId)
            throw error
        }
    }

    /** «v3.1.0-liquid-glass» → [3, 1, 0, 0]: суффикс после номера — метка ветки, не пре-релиз. */
    internal fun parseVersion(value: String): List<Int>? {
        val match = Regex("""\d+(\.\d+){1,3}""").find(value) ?: return null
        val parts = match.value.split('.').map { it.toIntOrNull() ?: return null }
        return parts + List(4 - parts.size) { 0 }
    }

    private fun compare(left: List<Int>, right: List<Int>): Int {
        for (index in 0 until 4) {
            val difference = left[index].compareTo(right[index])
            if (difference != 0) return difference
        }
        return 0
    }

    private fun label(version: List<Int>): String =
        (if (version[3] > 0) version else version.take(3)).joinToString(".")

    private fun findAsset(assets: JSONArray, name: String): JSONObject? {
        for (index in 0 until assets.length()) {
            val asset = assets.getJSONObject(index)
            if (asset.optString("name").equals(name, ignoreCase = true)) return asset
        }
        return null
    }

    private fun digestOf(asset: JSONObject): String? {
        val digest = asset.optString("digest")
        return if (digest.startsWith("sha256:", ignoreCase = true) && digest.length == 71) digest.substring(7) else null
    }

    private fun checksumFromList(assets: JSONArray, name: String): String? {
        val list = findAsset(assets, ChecksumsAsset) ?: return null
        return readText(list.getString("browser_download_url"), "text/plain").lineSequence()
            .map { it.trim().split(Regex("""\s+"""), limit = 2) }
            .firstOrNull { it.size == 2 && it[0].length == 64 && it[1].trimStart('*').equals(name, ignoreCase = true) }
            ?.get(0)
    }

    private fun readText(url: String, accept: String = "application/vnd.github+json"): String {
        val connection = open(url, accept)
        try {
            if (connection.responseCode !in 200..299) throw IOException("GitHub ответил ${connection.responseCode}")
            return connection.inputStream.bufferedReader().use { it.readText() }
        } finally {
            connection.disconnect()
        }
    }

    private fun open(url: String, accept: String): HttpURLConnection =
        (URL(url).openConnection() as HttpURLConnection).apply {
            connectTimeout = 15_000
            readTimeout = 30_000
            // GitHub API без User-Agent отвечает 403.
            setRequestProperty("User-Agent", "Turat/${BuildConfig.VERSION_NAME}")
            setRequestProperty("Accept", accept)
        }

    private fun sha256Of(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(1 shl 16)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().toHex()
    }

    private fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
}
