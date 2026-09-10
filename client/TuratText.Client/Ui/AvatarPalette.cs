using Avalonia;
using Avalonia.Media;

namespace TuratText.Client.Ui;

/// <summary>
/// Deterministic avatar gradients so every conversation keeps a stable, recognisable colour.
/// </summary>
public static class AvatarPalette
{
    private static readonly (string From, string To)[] Gradients =
    [
        ("#5B7BFF", "#9B6BFF"),
        ("#22C1DC", "#3B82F6"),
        ("#F97362", "#FFB454"),
        ("#35DDC0", "#2E9E86"),
        ("#B06BFF", "#FF6BC1"),
        ("#FFB454", "#F97362"),
        ("#4ADE80", "#12A594"),
        ("#7C8CFF", "#4C5EFF"),
        ("#FF6B7A", "#B04ACB"),
        ("#38BDF8", "#6C8BFF")
    ];

    private static readonly Dictionary<string, IBrush> Cache = new(StringComparer.Ordinal);

    public static IBrush For(string? key)
    {
        string id = string.IsNullOrEmpty(key) ? "?" : key;
        if (Cache.TryGetValue(id, out IBrush? cached)) return cached;

        (string from, string to) = Gradients[StableIndex(id, Gradients.Length)];
        IBrush brush = new LinearGradientBrush
        {
            StartPoint = new RelativePoint(0, 0, RelativeUnit.Relative),
            EndPoint = new RelativePoint(1, 1, RelativeUnit.Relative),
            GradientStops =
            {
                new GradientStop(Color.Parse(from), 0),
                new GradientStop(Color.Parse(to), 1)
            }
        }.ToImmutable();
        Cache[id] = brush;
        return brush;
    }

    private static int StableIndex(string value, int modulo)
    {
        unchecked
        {
            int hash = 17;
            foreach (char character in value) hash = hash * 31 + character;
            return Math.Abs(hash) % modulo;
        }
    }
}
