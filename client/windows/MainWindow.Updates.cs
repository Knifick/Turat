using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using TuratText.Windows.Updates;

namespace TuratText.Windows;

/// <summary>
/// Обновления из GitHub Releases. Интерфейс нарочно тихий: о новой версии говорит одна строка над
/// списком чатов, её можно скрыть до следующего запуска, а от самой версии — отказаться.
/// </summary>
public sealed partial class MainWindow
{
    private static readonly TimeSpan UpdateCheckInterval = TimeSpan.FromHours(6);
    private static readonly TimeSpan FirstUpdateCheckDelay = TimeSpan.FromSeconds(5);

    private readonly DispatcherTimer _updateTimer = new() { Interval = UpdateCheckInterval };
    private ReleaseInfo? _availableUpdate;
    private bool _updateBannerDismissed;
    private bool _checkingUpdates;
    private CancellationTokenSource? _updateDownload;

    private void StartUpdateChecks()
    {
        AppVersionText.Text = "Установлена версия " + GitHubUpdater.Label(GitHubUpdater.CurrentVersion);
        _updateTimer.Tick += async (_, _) => await CheckForUpdatesAsync(manual: false);
        _updateTimer.Start();
        _ = CheckForUpdatesAfterStartupAsync();
    }

    private void StopUpdates()
    {
        _updateTimer.Stop();
        _updateDownload?.Cancel();
    }

    /// <summary>Первая проверка чуть позже старта: сначала ядро и связь с Node.</summary>
    private async Task CheckForUpdatesAfterStartupAsync()
    {
        await Task.Delay(FirstUpdateCheckDelay);
        if (!_closing) await CheckForUpdatesAsync(manual: false);
    }

    /// <summary>
    /// Фоновая проверка молчит об ошибках: отсутствие GitHub не повод беспокоить пользователя.
    /// Ручная проверка показывает результат и снова предлагает даже пропущенную версию.
    /// </summary>
    private async Task CheckForUpdatesAsync(bool manual)
    {
        if (_checkingUpdates || _closing) return;
        _checkingUpdates = true;
        if (manual)
        {
            CheckUpdatesButton.IsEnabled = false;
            UpdateCheckStatus.Text = "Проверяем релизы на GitHub…";
        }
        try
        {
            ReleaseInfo? release = await GitHubUpdater.FindUpdateAsync();
            _availableUpdate = release;
            if (manual && release is not null)
            {
                _updateBannerDismissed = false;
                if (IsSkipped(release)) UiSettings.SkippedUpdateVersion = null;
            }
            UpdateCheckStatus.Text = release is null
                ? manual ? "У вас последняя версия." : string.Empty
                : $"Доступна версия {GitHubUpdater.Label(release.Version)}.";
        }
        catch (Exception exception) when (manual)
        {
            UpdateCheckStatus.Text = "Не удалось проверить обновления: " + exception.Message;
        }
        catch
        {
            // Фоновая проверка повторится по таймеру.
        }
        finally
        {
            _checkingUpdates = false;
            CheckUpdatesButton.IsEnabled = true;
            RefreshUpdateBanner();
        }
    }

    private static bool IsSkipped(ReleaseInfo release) =>
        UiSettings.SkippedUpdateVersion == release.Version.ToString();

    private void RefreshUpdateBanner()
    {
        ReleaseInfo? release = _availableUpdate;
        bool visible = release is not null && !_updateBannerDismissed && !IsSkipped(release);
        UpdateNotice.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
        ShowUpdateButton.Visibility = release is null ? Visibility.Collapsed : Visibility.Visible;
        if (release is not null)
        {
            UpdateBannerTitle.Text = $"Доступна версия {GitHubUpdater.Label(release.Version)}";
        }
    }

    private async void CheckUpdates_Click(object sender, RoutedEventArgs e) =>
        await CheckForUpdatesAsync(manual: true);

    private void DismissUpdate_Click(object sender, RoutedEventArgs e)
    {
        _updateBannerDismissed = true;
        RefreshUpdateBanner();
    }

    private async void OpenUpdate_Click(object sender, RoutedEventArgs e)
    {
        if (_availableUpdate is not ReleaseInfo release) return;
        UpdateDialog.Title = $"Turat {GitHubUpdater.Label(release.Version)}";
        UpdateMeta.Text =
            $"Сейчас установлена {GitHubUpdater.Label(GitHubUpdater.CurrentVersion)} · " +
            $"{release.Title} · {UpdateSizeLabel(release.Size)}";
        ReleaseNotes.Fill(UpdateNotes, release.Notes);
        UpdateProgress.Visibility = Visibility.Collapsed;
        UpdateStatus.Text = string.Empty;
        try
        {
            await UpdateDialog.ShowAsync();
        }
        catch (Exception)
        {
            // Уже открыт другой диалог: WinUI не показывает два сразу.
        }
    }

    private void OpenReleasePage_Click(object sender, RoutedEventArgs e)
    {
        if (_availableUpdate?.PageUrl is { Length: > 0 } url) GitHubUpdater.OpenInBrowser(url);
    }

    private void UpdateDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        // Диалог остаётся открытым: в нём виден прогресс загрузки и кнопка отмены.
        args.Cancel = true;
        _ = InstallUpdateAsync();
    }

    private void UpdateDialog_SecondaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        if (_availableUpdate is ReleaseInfo release) UiSettings.SkippedUpdateVersion = release.Version.ToString();
        RefreshUpdateBanner();
    }

    /// <summary>«Позже» прячет строку до следующего запуска, а во время загрузки отменяет её.</summary>
    private void UpdateDialog_CloseButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        if (_updateDownload is not null)
        {
            _updateDownload.Cancel();
            return;
        }
        _updateBannerDismissed = true;
        RefreshUpdateBanner();
    }

    private async Task InstallUpdateAsync()
    {
        if (_availableUpdate is not ReleaseInfo release || _updateDownload is not null) return;
        using var cancellation = new CancellationTokenSource();
        _updateDownload = cancellation;
        UpdateDialog.IsPrimaryButtonEnabled = false;
        UpdateDialog.IsSecondaryButtonEnabled = false;
        UpdateDialog.CloseButtonText = "Отменить";
        UpdateProgress.Value = 0;
        UpdateProgress.Visibility = Visibility.Visible;
        UpdateStatus.Text = "Загрузка…";
        var progress = new Progress<double>(fraction =>
        {
            UpdateProgress.Value = fraction * 100;
            UpdateStatus.Text =
                $"Загрузка… {UpdateSizeLabel((long)(fraction * release.Size))} из {UpdateSizeLabel(release.Size)}";
        });
        try
        {
            // Чтение сети и хеширование — вне потока интерфейса.
            string executable = await Task.Run(
                () => GitHubUpdater.DownloadAsync(release, progress, cancellation.Token), cancellation.Token);
            UpdateProgress.Value = 100;

            if (!GitHubUpdater.CanReplaceInPlace)
            {
                UpdateStatus.Text =
                    "Эта сборка состоит из нескольких файлов, поэтому заменить её автоматически нельзя. " +
                    "Проверенный Turat.exe открыт в проводнике — замените им текущий.";
                GitHubUpdater.RevealInExplorer(executable);
                return;
            }

            UpdateStatus.Text = "Контрольная сумма совпала. Перезапуск…";
            try
            {
                GitHubUpdater.ReplaceAndRestart(executable);
            }
            catch (Exception exception) when (exception is UnauthorizedAccessException or IOException)
            {
                UpdateStatus.Text =
                    "Нет прав на запись в папку приложения. Проверенный Turat.exe открыт в проводнике — " +
                    "замените им текущий вручную.";
                GitHubUpdater.RevealInExplorer(executable);
                return;
            }
            // Новая версия ждёт выхода этой: закрытие окна сохранит черновик и освободит ядро.
            _updateDownload = null;
            UpdateDialog.Hide();
            Close();
        }
        catch (OperationCanceledException)
        {
            UpdateStatus.Text = "Загрузка отменена.";
        }
        catch (Exception exception)
        {
            UpdateStatus.Text = "Не удалось обновиться: " + exception.Message;
        }
        finally
        {
            _updateDownload = null;
            UpdateDialog.IsPrimaryButtonEnabled = true;
            UpdateDialog.IsSecondaryButtonEnabled = true;
            UpdateDialog.CloseButtonText = "Позже";
        }
    }

    private static string UpdateSizeLabel(long bytes) => bytes >= 1024 * 1024
        ? $"{bytes / 1048576d:0.0} МБ"
        : $"{Math.Max(1, bytes / 1024)} КБ";
}
