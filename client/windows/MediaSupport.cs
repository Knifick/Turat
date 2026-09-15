using System.Globalization;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using TuratText.Windows.Media;
using Windows.Graphics.Imaging;
using Windows.Media.Core;
using Windows.Storage;
using Windows.Storage.FileProperties;
using Windows.Storage.Streams;
using Windows.System;

namespace TuratText.Windows;

/// <summary>
/// Работа с вложениями в окне: превью, просмотр, отправка и сохранение с понятным прогрессом.
/// </summary>
/// <remarks>
/// Тяжёлый файл больше не проходит через интерфейс целиком. Отправка запускается фоновой
/// задачей ядра и рисуется отдельной строкой ленты, сохранение — такой же задачей с полосой
/// над полем ввода, а просмотр читает зашифрованный файл потоково, поэтому видео стартует
/// сразу и перематывается без полной расшифровки.
/// </remarks>
public sealed partial class MainWindow
{
    /// <summary>Прогресс опрашивается чаще экрана, но заметно реже, чем идёт шифрование.</summary>
    private static readonly TimeSpan TransferPollInterval = TimeSpan.FromMilliseconds(120);

    /// <summary>
    /// Ниже этой стороны картинка считается пиксельной графикой: её увеличивают целым числом
    /// раз методом «ближайшего соседа», иначе спрайт превращается в мыло.
    /// </summary>
    private const int PixelArtEdge = 512;

    /// <summary>Ширина, до которой декодируется превью в пузыре.</summary>
    private const int BubblePreviewWidth = 640;

    /// <summary>Потолок разрешения при просмотре фотографии — больше ни один экран не покажет.</summary>
    private const int SmoothImageDecodeWidth = 2560;

    private readonly Dictionary<string, TransferModel> _transfers = [];
    private MessageModel? _viewedMessage;
    private string? _viewedImagePath;
    private int _viewedImageWidth;
    private int _viewedImageHeight;
    private int _viewedPixelFactor;
    private EncryptedMediaStream? _viewerStream;
    private string? _saveJobId;
    private CancellationTokenSource? _previewLoads;

    // --- отправка -------------------------------------------------------------

    private async void Attach_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedContactId is not string userId) return;
        StorageFile? file = await OpenFileAsync(["*"]);
        if (file is null) return;

        AttachmentMetadata media = await DescribeAsync(file);
        string mimeType = file.ContentType is { Length: > 0 } type ? type : "application/octet-stream";

        // Фото и видео можно отправить двумя способами, и выбор за пользователем: сжатое медиа
        // или файл байт в байт. Всё остальное — всегда файл.
        bool visual = media.Kind is "image" or "video";
        bool asMedia = false;
        if (visual)
        {
            if (media.Size > MaximumMediaBytes)
            {
                await TryShowErrorAsync($"Медиа больше {FormatBytes(MaximumMediaBytes)}");
                return;
            }
            SendChoice choice = await AskSendChoiceAsync(media);
            if (choice == SendChoice.Cancel) return;
            asMedia = choice == SendChoice.Media;
        }

        if (!asMedia)
        {
            if (media.Size > MaximumFileBytes)
            {
                await TryShowErrorAsync($"Файл больше {FormatBytes(MaximumFileBytes)}");
                return;
            }
            // Получатель должен увидеть вложение таким, каким его отправили, а не пытаться
            // проиграть его в ленте.
            if (visual) media = media with { Kind = "file", ThumbnailBase64 = null };
        }

        string path = file.Path;
        if (asMedia)
        {
            (StorageFile? compressed, AttachmentMetadata description) = await CompressAsync(file, media);
            if (compressed is not null)
            {
                path = compressed.Path;
                media = description;
                mimeType = media.Kind == "image" ? "image/jpeg" : "video/mp4";
            }
        }

        CoreResponse started = await Task.Run(() => _core.Invoke(new
        {
            command = "start_attachment",
            user_id = userId,
            path,
            mime_type = mimeType,
            caption = (string?)null,
            reply_to_event_id = _replyToEventId,
            kind = media.Kind,
            width = media.Width,
            height = media.Height,
            duration_milliseconds = media.DurationMilliseconds,
            thumbnail_base64 = media.ThumbnailBase64,
        }));
        if (started.Snapshot is not null) ApplySnapshot(started.Snapshot);
        string jobId = started.Text("jobId");
        if (!started.Ok || jobId.Length == 0)
        {
            await TryShowErrorAsync(started.Error ?? "Не удалось начать отправку файла");
            return;
        }
        _replyToEventId = null;
        UpdateBanner();

        var transfer = new TransferModel(jobId, userId, media.FileName, media.Size);
        _transfers[jobId] = transfer;
        // Строка живёт в ленте только пока диалог тот же, что и у передачи.
        if (_snapshot?.SelectedContactId == userId) _feed.Add(transfer);

        (bool ok, string error) = await TrackAsync(jobId, (done, total) =>
        {
            transfer.Done = done;
            if (total > 0) transfer.Total = total;
        });
        _transfers.Remove(jobId);
        if (ok)
        {
            _feed.Remove(transfer);
            await ExecuteAsync(new { command = "finish_attachment", job_id = jobId });
            return;
        }
        // Отменённая передача исчезает сразу, сорвавшаяся — задерживается с текстом ошибки.
        if (error.Length == 0)
        {
            _feed.Remove(transfer);
            return;
        }
        transfer.Error = error;
        await Task.Delay(TimeSpan.FromSeconds(4));
        _feed.Remove(transfer);
    }

    private async void CancelTransfer_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not TransferModel transfer) return;
        _feed.Remove(transfer);
        _transfers.Remove(transfer.JobId);
        await Task.Run(() => _core.Invoke(new { command = "cancel_media_job", job_id = transfer.JobId }));
    }

    // --- сохранение -----------------------------------------------------------

    private async void SaveAttachment_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is MessageModel message) await SaveAttachmentAsync(message);
    }

    private async void SaveViewedMedia_Click(object sender, RoutedEventArgs e)
    {
        if (_viewedMessage is MessageModel message) await SaveAttachmentAsync(message);
    }

    private async void CancelSave_Click(object sender, RoutedEventArgs e)
    {
        if (_saveJobId is not string jobId) return;
        _saveJobId = null;
        await Task.Run(() => _core.Invoke(new { command = "cancel_media_job", job_id = jobId }));
    }

    private async Task SaveAttachmentAsync(MessageModel message)
    {
        if (message.Attachment is not AttachmentModel attachment) return;
        StorageFile? target = await SaveFileAsync(
            attachment.FileName, Path.GetExtension(attachment.FileName) is { Length: > 0 } extension
                ? extension
                : ".bin");
        if (target is null) return;

        CoreResponse started = await Task.Run(() => _core.Invoke(new
        {
            command = "start_export_attachment",
            event_id = message.EventId,
            destination_path = target.Path,
        }));
        string jobId = started.Text("jobId");
        if (!started.Ok || jobId.Length == 0)
        {
            await TryShowErrorAsync(started.Error ?? "Не удалось сохранить файл");
            return;
        }

        _saveJobId = jobId;
        SaveTitle.Text = attachment.FileName;
        SaveProgress.Value = 0;
        SaveDetail.Text = string.Empty;
        SavePanel.Visibility = Visibility.Visible;
        (bool ok, string error) = await TrackAsync(jobId, (done, total) =>
        {
            SaveProgress.Value = total > 0 ? Math.Clamp(done * 100d / total, 0, 100) : 0;
            SaveDetail.Text = $"{Formatting.Bytes(done)} из {Formatting.Bytes(total)}";
        });
        SavePanel.Visibility = Visibility.Collapsed;
        _saveJobId = null;
        StatusText.Text = ok ? "Файл сохранён" : error.Length > 0 ? error : "Сохранение отменено";
    }

    /// <summary>
    /// Следит за фоновой задачей ядра. Возвращает успех и текст ошибки; пустая ошибка при
    /// неуспехе означает, что задача была отменена и говорить о сбое не о чем.
    /// </summary>
    private async Task<(bool Ok, string Error)> TrackAsync(string jobId, Action<long, long> onProgress)
    {
        while (true)
        {
            CoreResponse polled = await Task.Run(
                () => _core.Invoke(new { command = "media_job", job_id = jobId }));
            if (!polled.Ok || polled.Value is null) return (false, string.Empty);
            onProgress(polled.Number("done"), polled.Number("total"));
            switch (polled.Text("state"))
            {
                case "done":
                    return (true, string.Empty);
                case "failed":
                    return (false, polled.Text("error"));
            }
            await Task.Delay(TransferPollInterval);
        }
    }

    // --- превью в ленте --------------------------------------------------------

    /// <summary>
    /// Догружает превью для видимых вложений. Лента строится сразу, а расшифровка идёт следом,
    /// поэтому открытие диалога с сотней фотографий не заставляет ждать ни секунды.
    /// </summary>
    private void LoadPreviews()
    {
        _previewLoads?.Cancel();
        _previewLoads = new CancellationTokenSource();
        CancellationToken token = _previewLoads.Token;
        MessageModel[] pending = [.. _feed.OfType<MessageModel>()
            .Where(value => value.Preview is null && value.MediaVisibility == Visibility.Visible)];
        if (pending.Length == 0) return;
        _ = LoadPreviewsAsync(pending, token);
    }

    private async Task LoadPreviewsAsync(IReadOnlyList<MessageModel> messages, CancellationToken token)
    {
        foreach (MessageModel message in messages)
        {
            if (token.IsCancellationRequested) return;
            if (message.Attachment is not AttachmentModel attachment) continue;
            // Миниатюра из самого сообщения появляется мгновенно и держит место в пузыре.
            if (Images.Decode(attachment.ThumbnailBase64) is { } thumbnail) message.Preview = thumbnail;
            BitmapImage? full = await DecodeAsync(attachment, token);
            if (token.IsCancellationRequested) return;
            if (full is not null) message.Preview = full;
        }
    }

    /// <summary>Полноразмерный кадр вложения читается потоково, без расшифрованной копии на диске.</summary>
    private async Task<BitmapImage?> DecodeAsync(AttachmentModel attachment, CancellationToken token)
    {
        if (attachment.Media != MediaKind.Image) return null;
        try
        {
            using EncryptedMediaStream? stream = await Task.Run(
                () => EncryptedMediaStream.TryOpen(_core, attachment.LocalPath), token);
            if (stream is null || token.IsCancellationRequested) return null;
            var image = new BitmapImage();
            // Пузырь узкий: большой оригинал незачем декодировать целиком. Мелкую картинку,
            // наоборот, не трогаем — растягивать её при декодировании только во вред.
            if (attachment.Width <= 0 || attachment.Width > BubblePreviewWidth)
            {
                image.DecodePixelWidth = BubblePreviewWidth;
                image.DecodePixelType = DecodePixelType.Logical;
            }
            await image.SetSourceAsync(stream.AsRandomAccessStream());
            return image;
        }
        catch (Exception)
        {
            return null;
        }
    }

    // --- просмотр --------------------------------------------------------------

    private async void OpenMedia_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not MessageModel message) return;
        if (message.Attachment is not AttachmentModel attachment) return;

        _viewedMessage = message;
        MediaTitle.Text = attachment.FileName;
        MediaSubtitle.Text = attachment.Media == MediaKind.Video && attachment.DurationMilliseconds > 0
            ? $"{attachment.SizeLabel} · {attachment.DurationLabel}"
            : attachment.SizeLabel;
        MediaViewer.Visibility = Visibility.Visible;
        MediaError.Visibility = Visibility.Collapsed;
        MediaBusy.Visibility = Visibility.Visible;
        MediaImageScroll.Visibility = Visibility.Collapsed;
        MediaPlayer.Visibility = Visibility.Collapsed;

        EncryptedMediaStream? stream = await Task.Run(
            () => EncryptedMediaStream.TryOpen(_core, attachment.LocalPath));
        if (stream is null)
        {
            MediaBusy.Visibility = Visibility.Collapsed;
            MediaError.Text = "Не удалось открыть вложение";
            MediaError.Visibility = Visibility.Visible;
            return;
        }
        ReleaseViewerStream();

        try
        {
            if (attachment.Media == MediaKind.Image)
            {
                // Картинку декодируют целиком, поэтому поток нужен только на это время.
                await ShowImageAsync(attachment, stream);
            }
            else
            {
                // Плееру, наоборот, поток нужен до конца просмотра: он сам решает, что читать,
                // и расшифровываются только запрошенные им куски.
                _viewerStream = stream;
                MediaPlayer.Source = MediaSource.CreateFromStream(
                    stream.AsRandomAccessStream(), attachment.MimeType);
                MediaPlayer.Visibility = Visibility.Visible;
                MediaPlayer.MediaPlayer.Play();
            }
        }
        catch (Exception exception)
        {
            MediaError.Text = exception.Message;
            MediaError.Visibility = Visibility.Visible;
        }
        finally
        {
            MediaBusy.Visibility = Visibility.Collapsed;
        }
    }

    /// <summary>
    /// Показывает картинку так, чтобы она целиком помещалась в окно.
    /// </summary>
    /// <remarks>
    /// ScrollViewer меряет содержимое бесконечной шириной, поэтому снимок с телефона открывался
    /// в натуральную величину — заметно больше экрана. Ограничение по видимой области возвращает
    /// привычное поведение просмотрщика: сначала целиком, увеличение — уже руками.
    /// Мелкая графика — отдельный случай: её увеличивает <see cref="ApplyPixelArtAsync"/>.
    /// </remarks>
    private async Task ShowImageAsync(AttachmentModel attachment, EncryptedMediaStream probe)
    {
        // Размеры читаются по заголовку, и на этом поток исчерпан: дальше каждая ветка
        // открывает вложение заново. Раньше сюда же уходил и показ, поэтому большое фото
        // получало пустой поток и экран оставался чёрным.
        using (probe)
        {
            BitmapDecoder decoder = await BitmapDecoder.CreateAsync(probe.AsRandomAccessStream());
            _viewedImageWidth = (int)decoder.PixelWidth;
            _viewedImageHeight = (int)decoder.PixelHeight;
        }
        _viewedImagePath = attachment.LocalPath;
        _viewedPixelFactor = 0;
        MediaImageScroll.Visibility = Visibility.Visible;
        // Размеры области известны только после того, как просмотр стал видимым.
        MediaImageScroll.UpdateLayout();

        if (IsPixelArt(_viewedImageWidth, _viewedImageHeight))
        {
            await ApplyPixelArtAsync();
        }
        else
        {
            await ApplySmoothImageAsync();
        }
        MediaImageScroll.ChangeView(null, null, 1f, true);
    }

    /// <summary>Обычная фотография: декодируется с разумным потолком и вписывается в окно.</summary>
    private async Task ApplySmoothImageAsync()
    {
        if (_viewedImagePath is not string path) return;
        using EncryptedMediaStream? source = await Task.Run(
            () => EncryptedMediaStream.TryOpen(_core, path));
        if (source is null)
        {
            ShowMediaError("Не удалось открыть вложение");
            return;
        }
        var image = new BitmapImage();
        // Снимок на 6000 пикселей всё равно вписывается в окно, поэтому держать в памяти
        // его полное разрешение незачем; потолок с запасом переживает разворот окна.
        if (_viewedImageWidth > SmoothImageDecodeWidth)
        {
            image.DecodePixelType = DecodePixelType.Logical;
            image.DecodePixelWidth = SmoothImageDecodeWidth;
        }
        // Без этого сбой декодирования выглядел бы просто пустым экраном.
        image.ImageFailed += (_, failure) => ShowMediaError(failure.ErrorMessage);
        await image.SetSourceAsync(source.AsRandomAccessStream());
        MediaImage.Source = image;
        FitSmoothImage();
    }

    private void ShowMediaError(string message)
    {
        MediaError.Text = string.IsNullOrWhiteSpace(message) ? "Не удалось показать вложение" : message;
        MediaError.Visibility = Visibility.Visible;
    }

    private static bool IsPixelArt(int width, int height) =>
        width > 0 && height > 0 && Math.Min(width, height) < PixelArtEdge;

    /// <summary>Обычная фотография вписывается в окно средствами разметки, без пересчёта пикселей.</summary>
    private void FitSmoothImage()
    {
        (double width, double height) = ViewportSize();
        MediaImage.Stretch = Stretch.Uniform;
        MediaImage.Width = double.NaN;
        MediaImage.Height = double.NaN;
        MediaImage.MaxWidth = width;
        MediaImage.MaxHeight = height;
    }

    /// <summary>
    /// Увеличивает мелкую графику целым числом раз без сглаживания.
    /// </summary>
    /// <remarks>
    /// Множитель считается в физических пикселях и применяется к размеру в аппаратно-независимых
    /// единицах, поэтому на экране с масштабом 125% или 150% спрайт остаётся ровно таким же
    /// чётким, как при 100%: одна точка исходника занимает целое число точек экрана.
    /// </remarks>
    private async Task ApplyPixelArtAsync()
    {
        if (_viewedImagePath is not string path) return;
        double rasterization = RasterizationScale();
        (double viewportWidth, double viewportHeight) = ViewportSize();
        int factor = PixelArtFactor(viewportWidth * rasterization, viewportHeight * rasterization);
        if (factor == _viewedPixelFactor) return;
        _viewedPixelFactor = factor;

        using EncryptedMediaStream? source = await Task.Run(
            () => EncryptedMediaStream.TryOpen(_core, path));
        if (source is null) return;
        BitmapDecoder decoder = await BitmapDecoder.CreateAsync(source.AsRandomAccessStream());
        var transform = new BitmapTransform
        {
            ScaledWidth = (uint)(_viewedImageWidth * factor),
            ScaledHeight = (uint)(_viewedImageHeight * factor),
            InterpolationMode = BitmapInterpolationMode.NearestNeighbor,
        };
        SoftwareBitmap bitmap = await decoder.GetSoftwareBitmapAsync(
            BitmapPixelFormat.Bgra8,
            BitmapAlphaMode.Premultiplied,
            transform,
            ExifOrientationMode.RespectExifOrientation,
            ColorManagementMode.DoNotColorManage);
        var target = new SoftwareBitmapSource();
        await target.SetBitmapAsync(bitmap);
        MediaImage.Source = target;
        // Точный размер вместо Uniform: иначе картинку ещё раз растянет уже с интерполяцией.
        MediaImage.Stretch = Stretch.Fill;
        MediaImage.MaxWidth = double.PositiveInfinity;
        MediaImage.MaxHeight = double.PositiveInfinity;
        MediaImage.Width = _viewedImageWidth * factor / rasterization;
        MediaImage.Height = _viewedImageHeight * factor / rasterization;
    }

    private int PixelArtFactor(double viewportWidth, double viewportHeight)
    {
        if (_viewedImageWidth <= 0 || _viewedImageHeight <= 0) return 1;
        double fit = Math.Min(
            viewportWidth / _viewedImageWidth,
            viewportHeight / _viewedImageHeight);
        return (int)Math.Clamp(Math.Floor(fit), 1, 64);
    }

    private double RasterizationScale()
    {
        double scale = MediaImage.XamlRoot?.RasterizationScale ?? 1;
        return scale > 0 ? scale : 1;
    }

    /// <summary>Видимая область просмотра; до первой раскладки её заменяет размер окна.</summary>
    private (double Width, double Height) ViewportSize()
    {
        double width = MediaImageScroll.ViewportWidth > 0
            ? MediaImageScroll.ViewportWidth
            : Root.ActualWidth;
        double height = MediaImageScroll.ViewportHeight > 0
            ? MediaImageScroll.ViewportHeight
            : Root.ActualHeight - 60;
        double padding = MediaImageScroll.Padding.Left + MediaImageScroll.Padding.Right;
        return (Math.Max(width - padding, 1), Math.Max(height - padding, 1));
    }

    /// <summary>Изменение размера окна снова вписывает картинку — и пересчитывает целый множитель.</summary>
    private async void MediaImageScroll_SizeChanged(object sender, SizeChangedEventArgs e)
    {
        if (MediaImageScroll.Visibility != Visibility.Visible || _viewedImagePath is null) return;
        if (IsPixelArt(_viewedImageWidth, _viewedImageHeight))
        {
            await ApplyPixelArtAsync();
        }
        else
        {
            FitSmoothImage();
        }
    }

    private void CloseMedia_Click(object sender, RoutedEventArgs e) => CloseMediaViewer();

    /// <summary>Esc закрывает просмотр вложения раньше, чем до него доберётся остальное окно.</summary>
    private void Root_PreviewKeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Escape || MediaViewer.Visibility != Visibility.Visible) return;
        CloseMediaViewer();
        e.Handled = true;
    }

    private void CloseMediaViewer()
    {
        if (MediaViewer.Visibility != Visibility.Visible) return;
        try
        {
            MediaPlayer.MediaPlayer?.Pause();
        }
        catch (Exception)
        {
            // Плеер мог не успеть создать сессию — закрытию это не мешает.
        }
        MediaPlayer.Source = null;
        MediaImage.Source = null;
        MediaViewer.Visibility = Visibility.Collapsed;
        _viewedMessage = null;
        _viewedImagePath = null;
        _viewedImageWidth = 0;
        _viewedImageHeight = 0;
        _viewedPixelFactor = 0;
        ReleaseViewerStream();
    }

    private void ReleaseViewerStream()
    {
        _viewerStream?.Dispose();
        _viewerStream = null;
    }

    // --- разбор выбранного файла ------------------------------------------------

    private sealed record AttachmentMetadata(
        string FileName,
        long Size,
        string Kind,
        int Width,
        int Height,
        long DurationMilliseconds,
        string? ThumbnailBase64);

    /// <summary>
    /// Размеры, длительность и миниатюра уходят вместе с сообщением, поэтому получатель видит
    /// превью и длительность ролика сразу, не дожидаясь расшифровки оригинала.
    /// </summary>
    /// <summary>Медиа клиент сжимает, поэтому исходник может быть крупным.</summary>
    private const long MaximumMediaBytes = 512L * 1024 * 1024;

    /// <summary>Файл уходит байт в байт, и предел на него заметно строже.</summary>
    private const long MaximumFileBytes = 100L * 1024 * 1024;

    /// <summary>Как пользователь решил отправить выбранное фото или видео.</summary>
    private enum SendChoice
    {
        Cancel,
        Media,
        File,
    }

    /// <summary>
    /// Спрашивает способ отправки. Пункт «как файл» остаётся видимым и когда файл слишком
    /// велик, но с подписью, объясняющей почему он недоступен: молча спрятанная кнопка
    /// выглядит как поломка.
    /// </summary>
    private async Task<SendChoice> AskSendChoiceAsync(AttachmentMetadata media)
    {
        bool video = media.Kind == "video";
        bool fileAllowed = media.Size <= MaximumFileBytes;
        string explanation = video
            ? "Как видео — ролик будет сжат и пойдёт быстрее. Как файл — исходник без изменений."
            : "Как фото — снимок будет сжат и пойдёт быстрее. Как файл — исходник без изменений.";
        string size = fileAllowed
            ? $"Размер: {FormatBytes(media.Size)}"
            : $"Размер: {FormatBytes(media.Size)} — файлом можно до {FormatBytes(MaximumFileBytes)}";

        var dialog = new ContentDialog
        {
            XamlRoot = Root.XamlRoot,
            RequestedTheme = Root.RequestedTheme,
            Title = video ? "Отправить видео" : "Отправить фото",
            Content = explanation + Environment.NewLine + Environment.NewLine + size,
            PrimaryButtonText = video ? "Как видео" : "Как фото",
            SecondaryButtonText = "Как файл",
            CloseButtonText = "Отмена",
            IsSecondaryButtonEnabled = fileAllowed,
            DefaultButton = ContentDialogButton.Primary,
        };
        return await dialog.ShowAsync() switch
        {
            ContentDialogResult.Primary => SendChoice.Media,
            ContentDialogResult.Secondary => SendChoice.File,
            _ => SendChoice.Cancel,
        };
    }

    /// <summary>
    /// Сжатие перед отправкой. Задачи в ядре ещё нет, поэтому на время перекодирования в ленте
    /// висит собственная строка с процентами: иначе выбранное видео просто пропало бы на минуту.
    /// </summary>
    private async Task<(StorageFile? File, AttachmentMetadata Description)> CompressAsync(
        StorageFile source,
        AttachmentMetadata media)
    {
        var row = new TransferModel("compress-" + Guid.NewGuid().ToString("N"), string.Empty, media.FileName, 100)
        {
            Error = "Сжатие…",
        };
        _feed.Add(row);
        try
        {
            StorageFile? produced = media.Kind == "image"
                ? await Compression.ImageAsync(source)
                : await Compression.VideoAsync(
                    source,
                    media.Width,
                    media.Height,
                    percent => row.Done = (long)Math.Round(percent),
                    CancellationToken.None);
            if (produced is null) return (null, media);
            return (produced, await DescribeAsync(produced) with { FileName = Renamed(media.FileName, media.Kind) });
        }
        finally
        {
            _feed.Remove(row);
        }
    }

    /// <summary>Имя остаётся узнаваемым, но расширение должно отвечать новому содержимому.</summary>
    private static string Renamed(string fileName, string kind)
    {
        string extension = kind == "image" ? ".jpg" : ".mp4";
        string baseName = Path.GetFileNameWithoutExtension(fileName);
        return (baseName.Length == 0 ? "media" : baseName) + extension;
    }

    private static string FormatBytes(long value)
    {
        const double Megabyte = 1024.0 * 1024.0;
        return value >= 1024 * Megabyte
            ? string.Format(CultureInfo.InvariantCulture, "{0:0.0} ГБ", value / (1024 * Megabyte))
            : string.Format(CultureInfo.InvariantCulture, "{0:0.0} МБ", value / Megabyte);
    }

    private static async Task<AttachmentMetadata> DescribeAsync(StorageFile file)
    {
        BasicProperties basic = await file.GetBasicPropertiesAsync();
        string type = file.ContentType ?? string.Empty;
        string kind = type.StartsWith("image/", StringComparison.OrdinalIgnoreCase) ? "image"
            : type.StartsWith("video/", StringComparison.OrdinalIgnoreCase) ? "video"
            : type.StartsWith("audio/", StringComparison.OrdinalIgnoreCase) ? "audio"
            : "file";
        int width = 0;
        int height = 0;
        long duration = 0;
        try
        {
            switch (kind)
            {
                case "image":
                    ImageProperties image = await file.Properties.GetImagePropertiesAsync();
                    width = (int)image.Width;
                    height = (int)image.Height;
                    break;
                case "video":
                    VideoProperties video = await file.Properties.GetVideoPropertiesAsync();
                    width = (int)video.Width;
                    height = (int)video.Height;
                    duration = (long)video.Duration.TotalMilliseconds;
                    break;
                case "audio":
                    MusicProperties music = await file.Properties.GetMusicPropertiesAsync();
                    duration = (long)music.Duration.TotalMilliseconds;
                    break;
            }
        }
        catch (Exception)
        {
            // Повреждённый контейнер: вложение уйдёт без размеров и длительности.
        }
        string? thumbnail = kind is "image" or "video" ? await ThumbnailAsync(file) : null;
        return new AttachmentMetadata(file.Name, (long)basic.Size, kind, width, height, duration, thumbnail);
    }

    /// <summary>Миниатюра едет вместе с сообщением, поэтому она нарочно мелкая.</summary>
    private static async Task<string?> ThumbnailAsync(StorageFile file)
    {
        try
        {
            using StorageItemThumbnail? thumbnail = await file.GetThumbnailAsync(ThumbnailMode.SingleItem, 240);
            if (thumbnail is null || thumbnail.Size == 0) return null;
            var buffer = new global::Windows.Storage.Streams.Buffer((uint)thumbnail.Size);
            await thumbnail.ReadAsync(buffer, (uint)thumbnail.Size, InputStreamOptions.None);
            using DataReader reader = DataReader.FromBuffer(buffer);
            byte[] bytes = new byte[buffer.Length];
            reader.ReadBytes(bytes);
            return Convert.ToBase64String(bytes);
        }
        catch (Exception)
        {
            return null;
        }
    }
}
