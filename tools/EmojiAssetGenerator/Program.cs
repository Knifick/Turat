using SkiaSharp;

if (args.Length != 2)
{
    Console.Error.WriteLine("Usage: EmojiAssetGenerator <font.ttf> <output-directory>");
    return 1;
}

string fontPath = Path.GetFullPath(args[0]);
string outputDirectory = Path.GetFullPath(args[1]);
Directory.CreateDirectory(outputDirectory);

Dictionary<string, string> reactions = new()
{
    ["heart"] = "\u2764\uFE0F",
    ["fire"] = "\U0001F525",
    ["ok-hand"] = "\U0001F44C",
    ["scream"] = "\U0001F631",
    ["cry"] = "\U0001F62D",
    ["skeptical"] = "\U0001F928",
    ["thumbs-up"] = "\U0001F44D",
    ["broken-heart"] = "\U0001F494"
};

using SKTypeface typeface = SKTypeface.FromFile(fontPath)
    ?? throw new InvalidOperationException($"Could not load emoji font: {fontPath}");

foreach ((string name, string emoji) in reactions)
{
    using var bitmap = new SKBitmap(96, 96, SKColorType.Bgra8888, SKAlphaType.Premul);
    using var canvas = new SKCanvas(bitmap);
    using var paint = new SKPaint
    {
        Typeface = typeface,
        TextSize = 72,
        IsAntialias = true,
        TextAlign = SKTextAlign.Left,
        Color = SKColors.White
    };

    canvas.Clear(SKColors.Transparent);
    var bounds = new SKRect();
    paint.MeasureText(emoji, ref bounds);
    float x = 48 - bounds.MidX;
    float baseline = 48 - bounds.MidY;
    canvas.DrawText(emoji, x, baseline, paint);

    using SKImage image = SKImage.FromBitmap(bitmap);
    using SKData data = image.Encode(SKEncodedImageFormat.Png, 100);
    using FileStream output = File.Create(Path.Combine(outputDirectory, $"{name}.png"));
    data.SaveTo(output);
}

return 0;
