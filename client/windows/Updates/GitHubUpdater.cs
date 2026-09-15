using System.Diagnostics;
using System.IO.Compression;
using System.Net.Http.Headers;
using System.Security.Cryptography;
using System.Text.Json;
using System.Text.RegularExpressions;

namespace TuratText.Windows.Updates;

/// <summary>Опубликованный релиз, который новее установленной версии.</summary>
public sealed record ReleaseInfo(
    Version Version,
    string Tag,
    string Title,
    string Notes,
    string PageUrl,
    string AssetName,
    string DownloadUrl,
    long Size,
    string Sha256);

/// <summary>
/// Обновления из GitHub Releases официального репозитория.
/// </summary>
/// <remarks>
/// Целостность загрузки проверяется SHA-256: GitHub сам публикует хеш каждого файла релиза, а на
/// случай старых релизов без него берётся <c>SHA256SUMS.txt</c> того же релиза. Файл без
/// известного хеша не устанавливается.
/// </remarks>
internal static partial class GitHubUpdater
{
    public const string Owner = "Knifick";
    public const string Repository = "Turat";

    private const string ReleasesUrl = $"https://api.github.com/repos/{Owner}/{Repository}/releases?per_page=15";
    private const string ZipAssetName = "Turat-win-x64.zip";
    private const string ExeAssetName = "Turat.exe";
    private const string ChecksumsAssetName = "SHA256SUMS.txt";

    /// <summary>Каталог загрузок рядом с остальными локальными данными клиента.</summary>
    public static string DownloadDirectory { get; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "TuratText", "updates");

    public static Version CurrentVersion { get; } = Normalize(
        typeof(GitHubUpdater).Assembly.GetName().Version ?? new Version(0, 0, 0));

    // Объявлен после CurrentVersion: статические поля инициализируются по порядку, а заголовок
    // User-Agent (GitHub API без него отвечает 403) содержит номер версии.
    private static readonly HttpClient Http = CreateClient();

    /// <summary>Версия для людей: «3.1.0», а четвёртый компонент — только если он не нулевой.</summary>
    public static string Label(Version version) => version.Revision > 0
        ? version.ToString(4)
        : version.ToString(3);

    /// <summary>
    /// Самый свежий стабильный релиз новее установленного или <c>null</c>, если обновляться некуда.
    /// </summary>
    public static async Task<ReleaseInfo?> FindUpdateAsync(CancellationToken cancellation = default)
    {
        using HttpResponseMessage response = await Http.GetAsync(ReleasesUrl, cancellation);
        response.EnsureSuccessStatusCode();
        await using Stream body = await response.Content.ReadAsStreamAsync(cancellation);
        using JsonDocument document = await JsonDocument.ParseAsync(body, cancellationToken: cancellation);

        ReleaseInfo? best = null;
        foreach (JsonElement release in document.RootElement.EnumerateArray())
        {
            if (release.GetProperty("draft").GetBoolean() || release.GetProperty("prerelease").GetBoolean()) continue;
            string tag = release.GetProperty("tag_name").GetString() ?? string.Empty;
            if (ParseVersion(tag) is not Version version || version <= CurrentVersion) continue;
            if (best is not null && version <= best.Version) continue;

            JsonElement assets = release.GetProperty("assets");
            JsonElement? asset = FindAsset(assets, ZipAssetName) ?? FindAsset(assets, ExeAssetName);
            if (asset is not JsonElement chosen) continue;

            string name = chosen.GetProperty("name").GetString()!;
            string? sha256 = DigestOf(chosen)
                             ?? await ChecksumFromListAsync(assets, name, cancellation);
            if (sha256 is null) continue;

            best = new ReleaseInfo(
                version,
                tag,
                StringOrEmpty(release, "name") is { Length: > 0 } title ? title : tag,
                StringOrEmpty(release, "body"),
                StringOrEmpty(release, "html_url"),
                name,
                chosen.GetProperty("browser_download_url").GetString()!,
                chosen.GetProperty("size").GetInt64(),
                sha256);
        }
        return best;
    }

    /// <summary>
    /// Скачивает файл релиза, сверяет SHA-256 и возвращает путь к готовому <c>Turat.exe</c>.
    /// Уже скачанный и проверенный файл повторно не загружается.
    /// </summary>
    public static async Task<string> DownloadAsync(
        ReleaseInfo release, IProgress<double> progress, CancellationToken cancellation)
    {
        string directory = Path.Combine(DownloadDirectory, release.Version.ToString());
        Directory.CreateDirectory(directory);
        string target = Path.Combine(directory, release.AssetName);

        if (!File.Exists(target) || !string.Equals(await HashFileAsync(target, cancellation), release.Sha256,
                StringComparison.OrdinalIgnoreCase))
        {
            string partial = target + ".part";
            using (HttpResponseMessage response = await Http.GetAsync(
                       release.DownloadUrl, HttpCompletionOption.ResponseHeadersRead, cancellation))
            {
                response.EnsureSuccessStatusCode();
                long total = response.Content.Headers.ContentLength ?? release.Size;
                using var hash = IncrementalHash.CreateHash(HashAlgorithmName.SHA256);
                await using (Stream input = await response.Content.ReadAsStreamAsync(cancellation))
                await using (var output = new FileStream(partial, FileMode.Create, FileAccess.Write, FileShare.None,
                                 1 << 16, useAsync: true))
                {
                    byte[] buffer = new byte[1 << 16];
                    long done = 0;
                    double reported = -1;
                    int read;
                    while ((read = await input.ReadAsync(buffer, cancellation)) > 0)
                    {
                        await output.WriteAsync(buffer.AsMemory(0, read), cancellation);
                        hash.AppendData(buffer, 0, read);
                        done += read;
                        double fraction = total > 0 ? Math.Min(1d, (double)done / total) : 0;
                        // Полпроцента на шаг: иначе интерфейс получал бы тысячи обновлений.
                        if (fraction - reported < 0.005) continue;
                        reported = fraction;
                        progress.Report(fraction);
                    }
                }
                string actual = Convert.ToHexString(hash.GetHashAndReset());
                if (!string.Equals(actual, release.Sha256, StringComparison.OrdinalIgnoreCase))
                {
                    File.Delete(partial);
                    throw new InvalidDataException("Контрольная сумма загруженного файла не совпала с опубликованной.");
                }
            }
            File.Move(partial, target, overwrite: true);
        }

        if (!release.AssetName.EndsWith(".zip", StringComparison.OrdinalIgnoreCase)) return target;

        string executable = Path.Combine(directory, ExeAssetName);
        using ZipArchive archive = ZipFile.OpenRead(target);
        ZipArchiveEntry entry = archive.Entries.FirstOrDefault(value =>
                                    string.Equals(value.Name, ExeAssetName, StringComparison.OrdinalIgnoreCase))
                                ?? throw new InvalidDataException("В архиве релиза нет Turat.exe.");
        entry.ExtractToFile(executable, overwrite: true);
        return executable;
    }

    /// <summary>
    /// Можно ли заменить исполняемый файл на месте: это возможно только для однофайловой сборки,
    /// где весь клиент — один <c>Turat.exe</c>.
    /// </summary>
    public static bool CanReplaceInPlace =>
        string.IsNullOrEmpty(typeof(GitHubUpdater).Assembly.Location)
        && Environment.ProcessPath is string path
        && string.Equals(Path.GetFileName(path), ExeAssetName, StringComparison.OrdinalIgnoreCase);

    /// <summary>
    /// Ставит новый <c>Turat.exe</c> на место текущего и запускает его.
    /// </summary>
    /// <remarks>
    /// Windows не даёт перезаписать запущенный exe, но разрешает его переименовать. Старый файл
    /// остаётся рядом как <c>Turat.exe.old</c> и удаляется новой версией после выхода этой.
    /// Новый процесс ждёт завершения текущего: оба не должны одновременно открывать хранилище.
    /// </remarks>
    public static void ReplaceAndRestart(string newExecutable)
    {
        string current = Environment.ProcessPath ?? throw new InvalidOperationException("Не найден путь приложения.");
        string old = current + ".old";
        if (File.Exists(old)) File.Delete(old);
        File.Move(current, old);
        try
        {
            File.Copy(newExecutable, current);
        }
        catch
        {
            File.Move(old, current);
            throw;
        }

        var start = new ProcessStartInfo(current) { UseShellExecute = false };
        start.ArgumentList.Add(AfterUpdateArgument);
        start.ArgumentList.Add(Environment.ProcessId.ToString());
        Process.Start(start);
    }

    public const string AfterUpdateArgument = "--after-update";

    /// <summary>
    /// Запуск после обновления: дожидается выхода прежней версии и убирает её файл вместе с
    /// загрузками. Ошибки не мешают запуску — мусор уберёт следующий старт.
    /// </summary>
    public static void CompletePendingUpdate(string[] args)
    {
        int index = Array.IndexOf(args, AfterUpdateArgument);
        if (index >= 0 && index + 1 < args.Length && int.TryParse(args[index + 1], out int previous))
        {
            try
            {
                using Process process = Process.GetProcessById(previous);
                process.WaitForExit(30_000);
            }
            catch
            {
                // Процесс уже завершился.
            }
        }

        if (Environment.ProcessPath is not string path) return;
        string old = path + ".old";
        for (int attempt = 0; attempt < 10 && File.Exists(old); attempt++)
        {
            try
            {
                File.Delete(old);
            }
            catch
            {
                Thread.Sleep(300);
            }
        }
        if (index >= 0)
        {
            try
            {
                Directory.Delete(DownloadDirectory, recursive: true);
            }
            catch
            {
                // Загрузки — только кэш.
            }
        }
    }

    /// <summary>Открывает проводник на скачанном файле, когда заменить приложение на месте нельзя.</summary>
    public static void RevealInExplorer(string path) =>
        Process.Start(new ProcessStartInfo("explorer.exe", $"/select,\"{path}\"") { UseShellExecute = true });

    public static void OpenInBrowser(string url) =>
        Process.Start(new ProcessStartInfo(url) { UseShellExecute = true });

    /// <summary>«v3.1.0-liquid-glass» → 3.1.0: суффикс после номера — метка ветки, не пре-релиз.</summary>
    internal static Version? ParseVersion(string tag)
    {
        Match match = VersionPattern().Match(tag);
        return match.Success && Version.TryParse(match.Value, out Version? version) ? Normalize(version) : null;
    }

    /// <summary>Недостающие компоненты считаются нулями, иначе 3.0 и 3.0.0 оказались бы разными версиями.</summary>
    private static Version Normalize(Version version) => new(
        Math.Max(0, version.Major), Math.Max(0, version.Minor), Math.Max(0, version.Build), Math.Max(0, version.Revision));

    [GeneratedRegex(@"\d+(\.\d+){1,3}")]
    private static partial Regex VersionPattern();

    private static JsonElement? FindAsset(JsonElement assets, string name)
    {
        foreach (JsonElement asset in assets.EnumerateArray())
        {
            if (string.Equals(asset.GetProperty("name").GetString(), name, StringComparison.OrdinalIgnoreCase))
                return asset;
        }
        return null;
    }

    private static string? DigestOf(JsonElement asset)
    {
        string digest = StringOrEmpty(asset, "digest");
        const string prefix = "sha256:";
        return digest.StartsWith(prefix, StringComparison.OrdinalIgnoreCase) && digest.Length == prefix.Length + 64
            ? digest[prefix.Length..]
            : null;
    }

    private static async Task<string?> ChecksumFromListAsync(
        JsonElement assets, string name, CancellationToken cancellation)
    {
        if (FindAsset(assets, ChecksumsAssetName) is not JsonElement list) return null;
        string text = await Http.GetStringAsync(list.GetProperty("browser_download_url").GetString(), cancellation);
        foreach (string line in text.Split('\n'))
        {
            string[] parts = line.Trim().Split(' ', 2, StringSplitOptions.TrimEntries);
            if (parts.Length == 2 && parts[0].Length == 64
                                  && string.Equals(parts[1].TrimStart('*'), name, StringComparison.OrdinalIgnoreCase))
                return parts[0];
        }
        return null;
    }

    private static string StringOrEmpty(JsonElement element, string property) =>
        element.TryGetProperty(property, out JsonElement value) && value.ValueKind == JsonValueKind.String
            ? value.GetString() ?? string.Empty
            : string.Empty;

    private static async Task<string> HashFileAsync(string path, CancellationToken cancellation)
    {
        await using FileStream stream = File.OpenRead(path);
        return Convert.ToHexString(await SHA256.HashDataAsync(stream, cancellation));
    }

    private static HttpClient CreateClient()
    {
        var client = new HttpClient { Timeout = TimeSpan.FromMinutes(30) };
        client.DefaultRequestHeaders.UserAgent.Add(new ProductInfoHeaderValue("Turat", Label(CurrentVersion)));
        client.DefaultRequestHeaders.Accept.Add(new MediaTypeWithQualityHeaderValue("application/vnd.github+json"));
        return client;
    }
}
