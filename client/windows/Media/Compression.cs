using Windows.Graphics.Imaging;
using Windows.Media.MediaProperties;
using Windows.Media.Transcoding;
using Windows.Storage;
using Windows.Storage.Streams;

namespace TuratText.Windows.Media;

/// <summary>
/// Сжатие фото и видео перед отправкой.
/// </summary>
/// <remarks>
/// Сжатие с потерями, но умеренное: цель — убрать вес, которого никто не заметит, а не выжать
/// минимальный размер. Кадр уменьшается только если он крупнее нужного, а результат уходит
/// лишь тогда, когда он реально меньше оригинала: иначе отправляется исходный файл.
///
/// Кому нужен файл байт в байт, выбирает при отправке «как файл» — туда сжатие не заходит.
/// </remarks>
internal static class Compression
{
    /// <summary>Длинная сторона фотографии: 2560 точек хватает и для экрана, и чтобы приблизить.</summary>
    private const uint ImageLongestEdge = 2560;

    /// <summary>85 — порог, ниже которого JPEG начинает мылить лица и градиенты.</summary>
    private const double ImageQuality = 0.85;

    /// <summary>Короткая сторона видео: 720p остаётся нормальным качеством.</summary>
    private const int VideoShortSide = 720;
    private const uint VideoBitrate = 2_500_000;

    /// <summary>
    /// Пережимает фотографию в JPEG. Возвращает готовый файл или <c>null</c>, если сжимать
    /// нечего или не вышло, — вызывающий тогда отправляет оригинал.
    /// </summary>
    /// <remarks>
    /// Картинки с прозрачностью не трогаются: JPEG её не умеет, и прозрачный фон стал бы чёрным.
    /// </remarks>
    internal static async Task<StorageFile?> ImageAsync(StorageFile source)
    {
        StorageFile? target = null;
        try
        {
            using IRandomAccessStream input = await source.OpenAsync(FileAccessMode.Read);
            BitmapDecoder decoder = await BitmapDecoder.CreateAsync(input);
            if (decoder.OrientedPixelWidth == 0 || decoder.OrientedPixelHeight == 0) return null;

            uint longest = Math.Max(decoder.OrientedPixelWidth, decoder.OrientedPixelHeight);
            double ratio = longest > ImageLongestEdge ? (double)ImageLongestEdge / longest : 1.0;
            var transform = new BitmapTransform
            {
                ScaledWidth = Math.Max(1u, (uint)Math.Round(decoder.OrientedPixelWidth * ratio)),
                ScaledHeight = Math.Max(1u, (uint)Math.Round(decoder.OrientedPixelHeight * ratio)),
                InterpolationMode = BitmapInterpolationMode.Fant,
            };

            // RespectExifOrientation: поворот снимка лежит в метаданных, и без него портрет
            // после перекодирования лёг бы набок.
            using SoftwareBitmap bitmap = await decoder.GetSoftwareBitmapAsync(
                BitmapPixelFormat.Bgra8,
                BitmapAlphaMode.Premultiplied,
                transform,
                ExifOrientationMode.RespectExifOrientation,
                ColorManagementMode.ColorManageToSRgb);

            target = await ApplicationData.Current.TemporaryFolder.CreateFileAsync(
                "compressed-" + Guid.NewGuid().ToString("N") + ".jpg",
                CreationCollisionOption.ReplaceExisting);
            using (IRandomAccessStream output = await target.OpenAsync(FileAccessMode.ReadWrite))
            {
                var options = new BitmapPropertySet
                {
                    { "ImageQuality", new BitmapTypedValue(ImageQuality, global::Windows.Foundation.PropertyType.Single) },
                };
                BitmapEncoder encoder = await BitmapEncoder.CreateAsync(
                    BitmapEncoder.JpegEncoderId, output, options);
                // JPEG не хранит альфу, поэтому кадр отдаётся без неё.
                using SoftwareBitmap opaque = SoftwareBitmap.Convert(
                    bitmap, BitmapPixelFormat.Bgra8, BitmapAlphaMode.Ignore);
                encoder.SetSoftwareBitmap(opaque);
                await encoder.FlushAsync();
            }

            return await SmallerOrNullAsync(source, target);
        }
        catch (Exception)
        {
            await DeleteAsync(target);
            return null;
        }
    }

    /// <summary>
    /// Перекодирует видео в H.264/AAC с ограничением стороны кадра и битрейта.
    /// </summary>
    internal static async Task<StorageFile?> VideoAsync(
        StorageFile source,
        int width,
        int height,
        Action<double> onProgress,
        CancellationToken token)
    {
        StorageFile? target = null;
        try
        {
            MediaEncodingProfile profile = MediaEncodingProfile.CreateMp4(VideoEncodingQuality.HD720p);
            int shortSide = width > 0 && height > 0 ? Math.Min(width, height) : 0;
            // Кадр уменьшаем только если он крупнее цели: растянуть 480p до 720p значит
            // одновременно потерять качество и прибавить вес.
            if (shortSide > 0 && shortSide <= VideoShortSide)
            {
                profile.Video.Width = Even(width);
                profile.Video.Height = Even(height);
            }
            profile.Video.Bitrate = VideoBitrate;

            target = await ApplicationData.Current.TemporaryFolder.CreateFileAsync(
                "compressed-" + Guid.NewGuid().ToString("N") + ".mp4",
                CreationCollisionOption.ReplaceExisting);

            var transcoder = new MediaTranscoder { HardwareAccelerationEnabled = true };
            PrepareTranscodeResult prepared = await transcoder.PrepareFileTranscodeAsync(source, target, profile);
            if (!prepared.CanTranscode)
            {
                await DeleteAsync(target);
                return null;
            }

            await prepared.TranscodeAsync().AsTask(token, new Progress<double>(onProgress));
            return await SmallerOrNullAsync(source, target);
        }
        catch (Exception)
        {
            await DeleteAsync(target);
            return null;
        }
    }

    /// <summary>
    /// Ролик или снимок с телефона часто уже хорошо сжат: отдать файл потяжелее — худшее из
    /// возможного, поэтому в таком случае возвращается <c>null</c> и уходит оригинал.
    /// </summary>
    private static async Task<StorageFile?> SmallerOrNullAsync(StorageFile source, StorageFile target)
    {
        ulong original = (await source.GetBasicPropertiesAsync()).Size;
        ulong produced = (await target.GetBasicPropertiesAsync()).Size;
        if (produced == 0 || produced >= original)
        {
            await DeleteAsync(target);
            return null;
        }
        return target;
    }

    private static async Task DeleteAsync(StorageFile? file)
    {
        if (file is null) return;
        try
        {
            await file.DeleteAsync(StorageDeleteOption.PermanentDelete);
        }
        catch (Exception)
        {
            // Временная папка всё равно чистится системой.
        }
    }

    /// <summary>Кодеры H.264 не принимают нечётную сторону кадра.</summary>
    private static uint Even(int value) => (uint)Math.Max(2, value - (value % 2));
}
