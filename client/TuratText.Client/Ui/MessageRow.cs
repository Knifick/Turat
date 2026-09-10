using System.Globalization;
using Avalonia;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Media.Imaging;
using TuratText.Client.Messaging.V2;

namespace TuratText.Client.Ui;

/// <summary>
/// View-facing wrapper around <see cref="LocalTextMessage"/>. It carries the layout decisions that
/// depend on neighbouring messages (day separators, sender grouping, bubble geometry) so the XAML
/// stays declarative and the list can be diffed instead of rebuilt on every poll.
/// </summary>
public sealed class MessageRow
{
    private static readonly CultureInfo Russian = CultureInfo.GetCultureInfo("ru-RU");

    public MessageRow(
        LocalTextMessage message,
        bool showDateSeparator,
        bool isGroupStart,
        bool isGroupEnd,
        Bitmap? imageThumbnail = null)
    {
        Message = message;
        ShowDateSeparator = showDateSeparator;
        IsGroupStart = isGroupStart;
        IsGroupEnd = isGroupEnd;
        ImageThumbnail = imageThumbnail;
        BodyText = StripAttachmentPlaceholder(message);
        Signature = BuildSignature();
    }

    /// <summary>
    /// Attachment events carry a generated "clip name (N bytes)" line as their text. The bubble draws
    /// a real file card instead, so only a user-written caption is worth repeating.
    /// </summary>
    private static string StripAttachmentPlaceholder(LocalTextMessage message)
    {
        string text = message.DisplayText;
        if (message.Attachment is not { } attachment) return text;

        string placeholder = $"📎 {attachment.FileName} ({attachment.PlaintextSize} bytes)";
        if (text.EndsWith(placeholder, StringComparison.Ordinal))
        {
            text = text[..^placeholder.Length];
        }
        return text.Trim();
    }

    public LocalTextMessage Message { get; }
    public bool ShowDateSeparator { get; }
    public bool IsGroupStart { get; }
    public bool IsGroupEnd { get; }

    public bool Outgoing => Message.Outgoing;
    public string DisplayText => Message.DisplayText;

    /// <summary>Message text with the generated attachment line removed.</summary>
    public string BodyText { get; }
    public bool ShowText => BodyText.Length > 0;

    public string TimeLabel => Message.TimeLabel;
    public bool HasAttachment => Message.HasAttachment;
    public string AttachmentName => Message.Attachment?.FileName ?? "";
    public string AttachmentSize => FormatSize(Message.Attachment?.PlaintextSize ?? 0);
    public string AttachmentMimeType => Message.Attachment?.MimeType ?? "";
    public bool IsImageAttachment => AttachmentMimeType.StartsWith("image/", StringComparison.OrdinalIgnoreCase);

    /// <summary>Decrypted preview, filled in asynchronously once available; null shows the file chip.</summary>
    public Bitmap? ImageThumbnail { get; }
    public bool HasImageThumbnail => ImageThumbnail is not null;
    public bool ShowFileChip => HasAttachment && !HasImageThumbnail;
    public bool HasReactions => Message.HasReactions;
    public string ReactionSummary => Message.ReactionSummary;
    public bool IsDeleted => Message.Deleted;
    public bool IsEdited => Message.Edited && !Message.Deleted;

    public string DateLabel => FormatDay(Message.CreatedAt.ToLocalTime());

    public HorizontalAlignment BubbleAlignment => Outgoing
        ? HorizontalAlignment.Right
        : HorizontalAlignment.Left;

    public IBrush BubbleBackground => Outgoing ? Palette.Outgoing : Palette.Incoming;

    public IBrush BubbleBorder => Outgoing ? Palette.OutgoingBorder : Palette.IncomingBorder;

    public IBrush TextBrush => Outgoing ? Palette.OutgoingText : Palette.IncomingText;

    public IBrush MetaBrush => Outgoing ? Palette.OutgoingMeta : Palette.IncomingMeta;

    public IBrush AttachmentBackground => Outgoing ? Palette.OutgoingChip : Palette.IncomingChip;

    public CornerRadius BubbleCornerRadius => Outgoing
        ? new CornerRadius(20, IsGroupStart ? 20 : 8, 8, 20)
        : new CornerRadius(IsGroupStart ? 20 : 8, 20, 20, 8);

    public Thickness RowMargin => new(0, ShowDateSeparator ? 0 : (IsGroupStart ? 8 : 0), 0, IsGroupEnd ? 4 : 0);

    /// <summary>Delivery glyph shown on outgoing bubbles only.</summary>
    public bool ShowState => Outgoing && !Message.Deleted;

    public string StateGlyph => Message.Read ? "✓✓" : Message.Delivered ? "✓✓" : "✓";

    public IBrush StateBrush => Message.Read ? Palette.ReadTick : Palette.OutgoingMeta;

    /// <summary>Cheap content fingerprint used to diff the rendered list against fresh history.</summary>
    public string Signature { get; }

    private string BuildSignature() => string.Join(
        '|',
        Message.EventId,
        Message.Text,
        Message.Deleted ? "1" : "0",
        Message.Edited ? "1" : "0",
        Message.Delivered ? "1" : "0",
        Message.Read ? "1" : "0",
        string.Concat(Message.Reactions),
        Message.Attachment?.FileName ?? "",
        HasImageThumbnail ? "1" : "0",
        ShowDateSeparator ? "1" : "0",
        IsGroupStart ? "1" : "0",
        IsGroupEnd ? "1" : "0");

    private static string FormatDay(DateTimeOffset value)
    {
        DateTime today = DateTime.Today;
        DateTime day = value.Date;
        if (day == today) return "Сегодня";
        if (day == today.AddDays(-1)) return "Вчера";
        return day.Year == today.Year
            ? value.ToString("d MMMM", Russian)
            : value.ToString("d MMMM yyyy", Russian);
    }

    private static string FormatSize(long bytes)
    {
        if (bytes <= 0) return "";
        string[] units = ["Б", "КБ", "МБ", "ГБ"];
        double size = bytes;
        int unit = 0;
        while (size >= 1024 && unit < units.Length - 1)
        {
            size /= 1024;
            unit++;
        }
        return size < 10 && unit > 0
            ? $"{size:0.0} {units[unit]}"
            : $"{Math.Round(size)} {units[unit]}";
    }

    private static class Palette
    {
        public static readonly IBrush Outgoing = new LinearGradientBrush
        {
            StartPoint = new RelativePoint(0, 0, RelativeUnit.Relative),
            EndPoint = new RelativePoint(1, 1, RelativeUnit.Relative),
            GradientStops =
            {
                new GradientStop(Color.Parse("#4B6DFF"), 0),
                new GradientStop(Color.Parse("#7B57F0"), 1)
            }
        }.ToImmutable();

        public static readonly IBrush Incoming = new SolidColorBrush(Color.Parse("#141C29")).ToImmutable();
        public static readonly IBrush OutgoingBorder = new SolidColorBrush(Color.Parse("#00000000")).ToImmutable();
        public static readonly IBrush IncomingBorder = new SolidColorBrush(Color.Parse("#212C3D")).ToImmutable();
        public static readonly IBrush OutgoingText = new SolidColorBrush(Color.Parse("#FFFFFF")).ToImmutable();
        public static readonly IBrush IncomingText = new SolidColorBrush(Color.Parse("#E8EEF9")).ToImmutable();
        public static readonly IBrush OutgoingMeta = new SolidColorBrush(Color.Parse("#C9D3FF")).ToImmutable();
        public static readonly IBrush IncomingMeta = new SolidColorBrush(Color.Parse("#7D8CA3")).ToImmutable();
        public static readonly IBrush OutgoingChip = new SolidColorBrush(Color.Parse("#33FFFFFF")).ToImmutable();
        public static readonly IBrush IncomingChip = new SolidColorBrush(Color.Parse("#1C2735")).ToImmutable();
        public static readonly IBrush ReadTick = new SolidColorBrush(Color.Parse("#7BF3D8")).ToImmutable();
    }
}
