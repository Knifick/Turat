using System.Text.Json;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Media.Imaging;
using Windows.Storage.Streams;

namespace TuratText.Windows;

/// <summary>Заголовок секции в боковом списке («Чаты», «Сообщения»).</summary>
public sealed class SectionHeader(string title)
{
    public string Title { get; } = title;
}

/// <summary>Аватар из base64: декодируется прямо в привязке шаблона.</summary>
public static class Images
{
    public static ImageSource? Decode(string? base64)
    {
        if (string.IsNullOrEmpty(base64)) return null;
        try
        {
            byte[] bytes = Convert.FromBase64String(base64);
            var stream = new InMemoryRandomAccessStream();
            using (var writer = new DataWriter(stream.GetOutputStreamAt(0)))
            {
                writer.WriteBytes(bytes);
                writer.StoreAsync().AsTask().GetAwaiter().GetResult();
            }
            stream.Seek(0);
            var image = new BitmapImage();
            image.SetSource(stream);
            return image;
        }
        catch
        {
            return null;
        }
    }
}

/// <summary>Разделитель дня, входящее и исходящее сообщение рисуются разными шаблонами.</summary>
public sealed partial class MessageTemplateSelector : DataTemplateSelector
{
    public DataTemplate? Day { get; set; }

    public DataTemplate? Incoming { get; set; }

    public DataTemplate? Outgoing { get; set; }

    protected override DataTemplate? SelectTemplateCore(object item) => item switch
    {
        DaySeparator => Day,
        MessageModel { Outgoing: true } => Outgoing,
        MessageModel => Incoming,
        _ => null,
    };

    protected override DataTemplate? SelectTemplateCore(object item, DependencyObject container) =>
        SelectTemplateCore(item);
}

/// <summary>Боковой список смешивает заголовки секций, чаты и найденные сообщения.</summary>
public sealed partial class SidebarTemplateSelector : DataTemplateSelector
{
    public DataTemplate? Header { get; set; }

    public DataTemplate? Chat { get; set; }

    public DataTemplate? Hit { get; set; }

    protected override DataTemplate? SelectTemplateCore(object item) => item switch
    {
        SectionHeader => Header,
        ChatModel => Chat,
        SearchHitModel => Hit,
        _ => null,
    };

    protected override DataTemplate? SelectTemplateCore(object item, DependencyObject container) =>
        SelectTemplateCore(item);
}

/// <summary>Настройки оформления живут рядом с локальным хранилищем: приложение не упаковано.</summary>
internal static class UiSettings
{
    private static readonly string Path = System.IO.Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "TuratText", "ui-settings.json");

    /// <summary>Идентификатор темы из <see cref="ThemeCatalog"/>.</summary>
    public static string ThemeId
    {
        get
        {
            try
            {
                if (!File.Exists(Path)) return ThemeCatalog.All[0].Id;
                using JsonDocument document = JsonDocument.Parse(File.ReadAllText(Path));
                return document.RootElement.TryGetProperty("theme", out JsonElement value)
                    ? value.GetString() ?? ThemeCatalog.All[0].Id
                    : ThemeCatalog.All[0].Id;
            }
            catch
            {
                return ThemeCatalog.All[0].Id;
            }
        }
        set
        {
            try
            {
                Directory.CreateDirectory(System.IO.Path.GetDirectoryName(Path)!);
                File.WriteAllText(Path, JsonSerializer.Serialize(new { theme = value }));
            }
            catch
            {
                // Оформление — не критичная настройка: ошибку записи можно проигнорировать.
            }
        }
    }
}
