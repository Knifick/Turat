using System.Globalization;
using System.Text;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.Documents;
using Avalonia.Media;

namespace TuratText.Client.Controls;

public sealed class EmojiTextBlock : TextBlock
{
    public static readonly StyledProperty<string?> EmojiTextProperty =
        AvaloniaProperty.Register<EmojiTextBlock, string?>(nameof(EmojiText));

    private static readonly FontFamily TextFont = new("Inter");
    private static readonly FontFamily EmojiFont = new(
        "avares://TuratText.Client/Assets/Fonts#Noto Color Emoji");

    public EmojiTextBlock()
    {
        AttachedToVisualTree += (_, _) => RebuildInlines(EmojiText);
    }

    public string? EmojiText
    {
        get => GetValue(EmojiTextProperty);
        set => SetValue(EmojiTextProperty, value);
    }

    protected override void OnPropertyChanged(AvaloniaPropertyChangedEventArgs change)
    {
        base.OnPropertyChanged(change);
        if (change.Property == EmojiTextProperty)
        {
            RebuildInlines(change.NewValue as string);
        }
    }

    private void RebuildInlines(string? text)
    {
        Inlines?.Clear();
        if (string.IsNullOrEmpty(text) || Inlines is null)
        {
            return;
        }

        TextElementEnumerator elements = StringInfo.GetTextElementEnumerator(text);
        var current = new StringBuilder();
        bool? currentIsEmoji = null;
        while (elements.MoveNext())
        {
            string element = elements.GetTextElement();
            bool isEmoji = IsEmojiElement(element);
            if (currentIsEmoji is not null && currentIsEmoji != isEmoji)
            {
                AddRun(current.ToString(), currentIsEmoji.Value);
                current.Clear();
            }
            current.Append(element);
            currentIsEmoji = isEmoji;
        }
        if (current.Length > 0)
        {
            AddRun(current.ToString(), currentIsEmoji == true);
        }
    }

    private void AddRun(string text, bool isEmoji)
    {
        Inlines!.Add(new Run
        {
            Text = text,
            FontFamily = isEmoji ? EmojiFont : TextFont
        });
    }

    private static bool IsEmojiElement(string element)
    {
        foreach (Rune rune in element.EnumerateRunes())
        {
            int value = rune.Value;
            if (value is >= 0x1F000 and <= 0x1FAFF
                or >= 0x2300 and <= 0x23FF
                or >= 0x2600 and <= 0x27BF
                or >= 0x2B00 and <= 0x2BFF
                or 0x200D or 0x20E3 or 0xFE0F)
            {
                return true;
            }
        }
        return false;
    }
}
