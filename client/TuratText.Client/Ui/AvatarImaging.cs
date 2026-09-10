using Avalonia.Media.Imaging;

namespace TuratText.Client.Ui;

/// <summary>
/// Downscales a picked image into a small PNG suitable for embedding directly in a signed profile
/// claim (there is no server-side blob storage for public avatars, so the whole point is to keep it
/// tiny). Tries progressively smaller sizes until the encoded PNG fits the budget.
/// </summary>
public static class AvatarImaging
{
    private const int MaxAvatarPngBytes = 120_000;
    private static readonly int[] MaxSidesToTry = [256, 160, 96];

    public static byte[]? EncodeDownscaled(Stream sourceImageStream)
    {
        using Bitmap original = new(sourceImageStream);
        foreach (int maxSide in MaxSidesToTry)
        {
            double scale = Math.Min(
                1.0,
                (double)maxSide / Math.Max(original.PixelSize.Width, original.PixelSize.Height));
            var targetSize = new Avalonia.PixelSize(
                Math.Max(1, (int)Math.Round(original.PixelSize.Width * scale)),
                Math.Max(1, (int)Math.Round(original.PixelSize.Height * scale)));
            using Bitmap scaled = original.CreateScaledBitmap(targetSize, BitmapInterpolationMode.HighQuality);
            using var buffer = new MemoryStream();
            scaled.Save(buffer);
            if (buffer.Length <= MaxAvatarPngBytes) return buffer.ToArray();
        }
        return null;
    }
}
