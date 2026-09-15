using System.Text.RegularExpressions;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Documents;

namespace TuratText.Windows.Updates;

/// <summary>
/// Ченджлог релиза пишется в Markdown. Полноценный рендерер ради одного окна не нужен: заголовки
/// становятся жирными строками, списки — пунктами, таблицы — парами «ячейка — ячейка».
/// </summary>
internal static partial class ReleaseNotes
{
    public static void Fill(TextBlock target, string markdown)
    {
        target.Inlines.Clear();
        bool pendingBreak = false;
        bool previousBlank = true;
        foreach (string raw in markdown.Replace("\r", string.Empty).Split('\n'))
        {
            string line = raw.Trim();
            if (line.Length == 0)
            {
                if (!previousBlank) pendingBreak = true;
                previousBlank = true;
                continue;
            }
            if (TableSeparator().IsMatch(line)) continue;

            if (target.Inlines.Count > 0)
            {
                target.Inlines.Add(new LineBreak());
                if (pendingBreak) target.Inlines.Add(new LineBreak());
            }
            pendingBreak = false;
            previousBlank = false;

            Match heading = Heading().Match(line);
            if (heading.Success)
            {
                target.Inlines.Add(new Run
                {
                    Text = Inline(heading.Groups[1].Value),
                    FontWeight = FontWeights.SemiBold,
                    FontSize = target.FontSize + 1,
                });
                continue;
            }
            if (line.StartsWith('|'))
            {
                string[] cells = line.Trim('|').Split('|', StringSplitOptions.TrimEntries | StringSplitOptions.RemoveEmptyEntries);
                target.Inlines.Add(new Run { Text = "• " + string.Join(" — ", cells.Select(Inline)) });
                continue;
            }
            Match bullet = Bullet().Match(line);
            target.Inlines.Add(new Run { Text = bullet.Success ? "• " + Inline(bullet.Groups[1].Value) : Inline(line) });
        }
        if (target.Inlines.Count == 0) target.Inlines.Add(new Run { Text = "Автор не приложил описание изменений." });
    }

    /// <summary>Ссылки превращаются в текст, служебные символы выделения убираются.</summary>
    private static string Inline(string text)
    {
        text = Link().Replace(text, "$1");
        return text.Replace("**", string.Empty).Replace("__", string.Empty).Replace("`", string.Empty);
    }

    [GeneratedRegex(@"^#{1,6}\s+(.*)$")]
    private static partial Regex Heading();

    [GeneratedRegex(@"^(?:[-*+]|\d+\.)\s+(.*)$")]
    private static partial Regex Bullet();

    [GeneratedRegex(@"^\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)*\|?$")]
    private static partial Regex TableSeparator();

    [GeneratedRegex(@"!?\[([^\]]*)\]\([^)]*\)")]
    private static partial Regex Link();
}
