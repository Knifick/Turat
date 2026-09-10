using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace TuratText.Windows;

/// <summary>
/// Одна тема оформления. Цвета описаны как ARGB-константы и раскладываются по кистям
/// приложения: разметка ссылается на ключи, а не на конкретные значения.
/// </summary>
public sealed record ThemePalette(
    string Id,
    string Title,
    bool Dark,
    uint Window,
    uint Panel,
    uint Rail,
    uint Elevated,
    uint Field,
    uint Divider,
    uint Text,
    uint Hint,
    uint Accent,
    uint OnAccent,
    uint AccentSoft,
    uint BubbleIn,
    uint BubbleOut,
    uint BubbleInText,
    uint BubbleOutText,
    uint MetaIn,
    uint MetaOut,
    uint QuoteIn,
    uint QuoteOut,
    uint Tick,
    uint ServicePill,
    uint ServiceText,
    uint Badge,
    uint BadgeMuted,
    uint BadgeText,
    uint Online,
    uint Danger,
    uint RowActive,
    uint RowHover,
    uint Selection,
    uint WallpaperTop,
    uint WallpaperBottom)
{
    public Brush Preview => Gradient(WallpaperTop, WallpaperBottom);

    public Brush AccentBrush => new SolidColorBrush(ThemeCatalog.ToColor(Accent));

    public Brush AccentSoftBrush => new SolidColorBrush(ThemeCatalog.ToColor(AccentSoft));

    public Brush TitleBrush => new SolidColorBrush(ThemeCatalog.ToColor(Text));

    public Brush BorderBrush => new SolidColorBrush(ThemeCatalog.ToColor(Divider));

    private static Brush Gradient(uint top, uint bottom)
    {
        var brush = new LinearGradientBrush { StartPoint = new(0, 0), EndPoint = new(0, 1) };
        brush.GradientStops.Add(new GradientStop { Color = ThemeCatalog.ToColor(top), Offset = 0 });
        brush.GradientStops.Add(new GradientStop { Color = ThemeCatalog.ToColor(bottom), Offset = 1 });
        return brush;
    }
}

/// <summary>
/// Каталог тем и их применение. Кисти из App.xaml создаются один раз, поэтому смена темы —
/// это присваивание новых <see cref="SolidColorBrush.Color"/>: всё уже нарисованное
/// перекрашивается само, пересоздавать окно не нужно.
/// </summary>
internal static class ThemeCatalog
{
    public static readonly IReadOnlyList<ThemePalette> All =
    [
        new("Origin", "Ориджин", true,
            Window: 0xFF1C1C1C, Panel: 0xFF1C1C1C, Rail: 0xFF171717, Elevated: 0xFF242424,
            Field: 0xFF2B2B2B, Divider: 0xFF353535, Text: 0xFFFFFFFF, Hint: 0xFF9A9A9A,
            Accent: 0xFFC9354A, OnAccent: 0xFFFFFFFF, AccentSoft: 0xFF572832,
            BubbleIn: 0xFF292929, BubbleOut: 0xFFC9354A, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFFFFFFFF,
            MetaIn: 0xFF969696, MetaOut: 0xFFFFD7DD, QuoteIn: 0x33C9354A, QuoteOut: 0x22FFFFFF,
            Tick: 0xFFFFD7DD, ServicePill: 0xCC242424, ServiceText: 0xFFB5B5B5,
            Badge: 0xFFC9354A, BadgeMuted: 0xFF5C5C5C, BadgeText: 0xFFFFFFFF,
            Online: 0xFF4CCC5E, Danger: 0xFFEC6A65, RowActive: 0xFF572832, RowHover: 0xFF262626,
            Selection: 0x40C9354A, WallpaperTop: 0xFF1C1C1C, WallpaperBottom: 0xFF211B1C),

        new("Garnet", "Гранат", true,
            Window: 0xFF1F0D12, Panel: 0xFF241016, Rail: 0xFF190A0E, Elevated: 0xFF2E151D,
            Field: 0xFF381A23, Divider: 0xFF44212B, Text: 0xFFFFFFFF, Hint: 0xFFB4909C,
            Accent: 0xFFD93E5C, OnAccent: 0xFFFFFFFF, AccentSoft: 0xFF5C2130,
            BubbleIn: 0xFF2E151D, BubbleOut: 0xFFD93E5C, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFFFFFFFF,
            MetaIn: 0xFFAD8894, MetaOut: 0xFFFFD8E0, QuoteIn: 0x33D93E5C, QuoteOut: 0x22FFFFFF,
            Tick: 0xFFFFD8E0, ServicePill: 0xCC2E151D, ServiceText: 0xFFC5A5AF,
            Badge: 0xFFD93E5C, BadgeMuted: 0xFF6B4550, BadgeText: 0xFFFFFFFF,
            Online: 0xFF4CCC5E, Danger: 0xFFFF8A80, RowActive: 0xFF5C2130, RowHover: 0xFF2E151D,
            Selection: 0x40D93E5C, WallpaperTop: 0xFF1F0D12, WallpaperBottom: 0xFF2A1119),

        new("Obsidian", "Обсидиан", true,
            Window: 0xFF0F1626, Panel: 0xFF131C2F, Rail: 0xFF0B1220, Elevated: 0xFF1A2440,
            Field: 0xFF1F2B4A, Divider: 0xFF283557, Text: 0xFFFFFFFF, Hint: 0xFF8E9BBA,
            Accent: 0xFF3B6FE0, OnAccent: 0xFFFFFFFF, AccentSoft: 0xFF23355F,
            BubbleIn: 0xFF1B2542, BubbleOut: 0xFF3B6FE0, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFFFFFFFF,
            MetaIn: 0xFF8C9AB8, MetaOut: 0xFFD3E1FF, QuoteIn: 0x333B6FE0, QuoteOut: 0x22FFFFFF,
            Tick: 0xFFD3E1FF, ServicePill: 0xCC1A2440, ServiceText: 0xFFA7B4CE,
            Badge: 0xFF3B6FE0, BadgeMuted: 0xFF4A5670, BadgeText: 0xFFFFFFFF,
            Online: 0xFF3ECF7E, Danger: 0xFFF2766B, RowActive: 0xFF23355F, RowHover: 0xFF1A2440,
            Selection: 0x403B6FE0, WallpaperTop: 0xFF0F1626, WallpaperBottom: 0xFF141D33),

        new("Black", "Чёрный", true,
            Window: 0xFF000000, Panel: 0xFF0A0A0A, Rail: 0xFF000000, Elevated: 0xFF141414,
            Field: 0xFF1B1B1B, Divider: 0xFF262626, Text: 0xFFFFFFFF, Hint: 0xFF8C8C8C,
            Accent: 0xFFE6E6E6, OnAccent: 0xFF0A0A0A, AccentSoft: 0xFF2A2A2A,
            BubbleIn: 0xFF171717, BubbleOut: 0xFFE6E6E6, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFF0A0A0A,
            MetaIn: 0xFF8C8C8C, MetaOut: 0xFF565656, QuoteIn: 0x33FFFFFF, QuoteOut: 0x22000000,
            Tick: 0xFF565656, ServicePill: 0xCC171717, ServiceText: 0xFFB0B0B0,
            Badge: 0xFFE6E6E6, BadgeMuted: 0xFF4A4A4A, BadgeText: 0xFF0A0A0A,
            Online: 0xFF4CCC5E, Danger: 0xFFFF6B60, RowActive: 0xFF232323, RowHover: 0xFF161616,
            Selection: 0x33FFFFFF, WallpaperTop: 0xFF000000, WallpaperBottom: 0xFF050505),

        new("Graphite", "Графит", true,
            Window: 0xFF1C242C, Panel: 0xFF202932, Rail: 0xFF171E25, Elevated: 0xFF29343F,
            Field: 0xFF2F3B47, Divider: 0xFF394653, Text: 0xFFFFFFFF, Hint: 0xFF9AAAB8,
            Accent: 0xFF6E93B8, OnAccent: 0xFF0D1720, AccentSoft: 0xFF31465A,
            BubbleIn: 0xFF29343F, BubbleOut: 0xFF6E93B8, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFF0D1720,
            MetaIn: 0xFF94A4B2, MetaOut: 0xFF2B4055, QuoteIn: 0x336E93B8, QuoteOut: 0x22000000,
            Tick: 0xFF2B4055, ServicePill: 0xCC29343F, ServiceText: 0xFFB0BECA,
            Badge: 0xFF6E93B8, BadgeMuted: 0xFF4A5A68, BadgeText: 0xFF0D1720,
            Online: 0xFF48C77A, Danger: 0xFFF07A6E, RowActive: 0xFF31465A, RowHover: 0xFF29343F,
            Selection: 0x406E93B8, WallpaperTop: 0xFF1C242C, WallpaperBottom: 0xFF212B35),

        new("Emerald", "Изумрудный", true,
            Window: 0xFF05231D, Panel: 0xFF072A23, Rail: 0xFF041D18, Elevated: 0xFF0B372E,
            Field: 0xFF104338, Divider: 0xFF175143, Text: 0xFFFFFFFF, Hint: 0xFF8DB6AA,
            Accent: 0xFF0F9D6F, OnAccent: 0xFFFFFFFF, AccentSoft: 0xFF13503E,
            BubbleIn: 0xFF0B372E, BubbleOut: 0xFF0F9D6F, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFFFFFFFF,
            MetaIn: 0xFF8AAFA4, MetaOut: 0xFFCDF3E4, QuoteIn: 0x330F9D6F, QuoteOut: 0x22FFFFFF,
            Tick: 0xFFCDF3E4, ServicePill: 0xCC0B372E, ServiceText: 0xFFA5C9BF,
            Badge: 0xFF0F9D6F, BadgeMuted: 0xFF3F6459, BadgeText: 0xFFFFFFFF,
            Online: 0xFF3FD98C, Danger: 0xFFF0796C, RowActive: 0xFF13503E, RowHover: 0xFF0B372E,
            Selection: 0x400F9D6F, WallpaperTop: 0xFF05231D, WallpaperBottom: 0xFF082B23),

        new("Ocean", "Океан", true,
            Window: 0xFF06222F, Panel: 0xFF082938, Rail: 0xFF051C27, Elevated: 0xFF0C3547,
            Field: 0xFF104155, Divider: 0xFF174F65, Text: 0xFFFFFFFF, Hint: 0xFF89AABB,
            Accent: 0xFF1E8FC4, OnAccent: 0xFFFFFFFF, AccentSoft: 0xFF124D66,
            BubbleIn: 0xFF0C3547, BubbleOut: 0xFF1E8FC4, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFFFFFFFF,
            MetaIn: 0xFF87A5B6, MetaOut: 0xFFCDEBFA, QuoteIn: 0x331E8FC4, QuoteOut: 0x22FFFFFF,
            Tick: 0xFFCDEBFA, ServicePill: 0xCC0C3547, ServiceText: 0xFFA2C1D2,
            Badge: 0xFF1E8FC4, BadgeMuted: 0xFF3D6274, BadgeText: 0xFFFFFFFF,
            Online: 0xFF3FD08C, Danger: 0xFFF0796C, RowActive: 0xFF124D66, RowHover: 0xFF0C3547,
            Selection: 0x401E8FC4, WallpaperTop: 0xFF06222F, WallpaperBottom: 0xFF082A39),

        new("Amber", "Янтарь", true,
            Window: 0xFF241407, Panel: 0xFF2B190A, Rail: 0xFF1D1005, Elevated: 0xFF382110,
            Field: 0xFF442915, Divider: 0xFF52331C, Text: 0xFFFFFFFF, Hint: 0xFFBBA189,
            Accent: 0xFFD98324, OnAccent: 0xFF1C0F02, AccentSoft: 0xFF5C3714,
            BubbleIn: 0xFF382110, BubbleOut: 0xFFD98324, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFF1C0F02,
            MetaIn: 0xFFB49A82, MetaOut: 0xFF56320C, QuoteIn: 0x33D98324, QuoteOut: 0x22000000,
            Tick: 0xFF56320C, ServicePill: 0xCC382110, ServiceText: 0xFFC9B39C,
            Badge: 0xFFD98324, BadgeMuted: 0xFF6B5238, BadgeText: 0xFF1C0F02,
            Online: 0xFF5CC96B, Danger: 0xFFF57F6C, RowActive: 0xFF5C3714, RowHover: 0xFF382110,
            Selection: 0x40D98324, WallpaperTop: 0xFF241407, WallpaperBottom: 0xFF2C1A0B),

        new("Amethyst", "Аметист", true,
            Window: 0xFF1B1030, Panel: 0xFF201438, Rail: 0xFF160C28, Elevated: 0xFF2A1B49,
            Field: 0xFF332158, Divider: 0xFF3D2968, Text: 0xFFFFFFFF, Hint: 0xFFA595C4,
            Accent: 0xFF8B5CF6, OnAccent: 0xFFFFFFFF, AccentSoft: 0xFF3B2670,
            BubbleIn: 0xFF2A1B49, BubbleOut: 0xFF8B5CF6, BubbleInText: 0xFFFFFFFF, BubbleOutText: 0xFFFFFFFF,
            MetaIn: 0xFFA294BE, MetaOut: 0xFFE4D8FF, QuoteIn: 0x338B5CF6, QuoteOut: 0x22FFFFFF,
            Tick: 0xFFE4D8FF, ServicePill: 0xCC2A1B49, ServiceText: 0xFFBAABD6,
            Badge: 0xFF8B5CF6, BadgeMuted: 0xFF574A73, BadgeText: 0xFFFFFFFF,
            Online: 0xFF4CD07F, Danger: 0xFFF4796F, RowActive: 0xFF3B2670, RowHover: 0xFF2A1B49,
            Selection: 0x408B5CF6, WallpaperTop: 0xFF1B1030, WallpaperBottom: 0xFF211539),

        new("Cloud", "Облачный", false,
            Window: 0xFFEEF2FB, Panel: 0xFFFFFFFF, Rail: 0xFFE4EAF7, Elevated: 0xFFFFFFFF,
            Field: 0xFFE7ECF7, Divider: 0xFFD8DFEF, Text: 0xFF16202E, Hint: 0xFF6A7A90,
            Accent: 0xFF2563EB, OnAccent: 0xFFFFFFFF, AccentSoft: 0xFFD6E2FB,
            BubbleIn: 0xFFFFFFFF, BubbleOut: 0xFF2563EB, BubbleInText: 0xFF16202E, BubbleOutText: 0xFFFFFFFF,
            MetaIn: 0xFF8494A8, MetaOut: 0xFFCBDDFF, QuoteIn: 0x332563EB, QuoteOut: 0x33FFFFFF,
            Tick: 0xFFCBDDFF, ServicePill: 0xE6DCE5F5, ServiceText: 0xFF5C6B80,
            Badge: 0xFF2563EB, BadgeMuted: 0xFFA6B2C4, BadgeText: 0xFFFFFFFF,
            Online: 0xFF1F9D55, Danger: 0xFFD93025, RowActive: 0xFFD6E2FB, RowHover: 0xFFE7ECF7,
            Selection: 0x332563EB, WallpaperTop: 0xFFEEF2FB, WallpaperBottom: 0xFFE5ECF9),

        new("Parchment", "Пергамент", false,
            Window: 0xFFF2EADA, Panel: 0xFFFBF5E9, Rail: 0xFFEBE0CB, Elevated: 0xFFFBF5E9,
            Field: 0xFFEDE3D0, Divider: 0xFFDED0B6, Text: 0xFF2E2418, Hint: 0xFF7C6C55,
            Accent: 0xFF96683A, OnAccent: 0xFFFFF8EC, AccentSoft: 0xFFE4D6BC,
            BubbleIn: 0xFFFBF5E9, BubbleOut: 0xFF96683A, BubbleInText: 0xFF2E2418, BubbleOutText: 0xFFFFF8EC,
            MetaIn: 0xFF9C8A70, MetaOut: 0xFFE7D3BB, QuoteIn: 0x3396683A, QuoteOut: 0x33FFFFFF,
            Tick: 0xFFE7D3BB, ServicePill: 0xE6E6DAC2, ServiceText: 0xFF6E5E48,
            Badge: 0xFF96683A, BadgeMuted: 0xFFB7A88F, BadgeText: 0xFFFFF8EC,
            Online: 0xFF4E8A3C, Danger: 0xFFB3381F, RowActive: 0xFFE4D6BC, RowHover: 0xFFEDE3D0,
            Selection: 0x3396683A, WallpaperTop: 0xFFF2EADA, WallpaperBottom: 0xFFEDE2CE),
    ];

    /// <summary>Неизвестный идентификатор из настроек прежних версий приводит к фирменной теме.</summary>
    public static ThemePalette Resolve(string? id) =>
        All.FirstOrDefault(theme => string.Equals(theme.Id, id, StringComparison.OrdinalIgnoreCase)) ?? All[0];

    public static Color ToColor(uint value) => Color.FromArgb(
        (byte)(value >> 24), (byte)(value >> 16), (byte)(value >> 8), (byte)value);

    public static void Apply(ThemePalette theme)
    {
        ResourceDictionary resources = Application.Current.Resources;
        Set(resources, "TgWindow", theme.Window);
        Set(resources, "TgPanel", theme.Panel);
        Set(resources, "TgRail", theme.Rail);
        Set(resources, "TgElevated", theme.Elevated);
        Set(resources, "TgField", theme.Field);
        Set(resources, "TgDivider", theme.Divider);
        Set(resources, "TgText", theme.Text);
        Set(resources, "TgHint", theme.Hint);
        Set(resources, "TgAccent", theme.Accent);
        Set(resources, "TgOnAccent", theme.OnAccent);
        Set(resources, "TgAccentSoft", theme.AccentSoft);
        Set(resources, "TgBubbleIn", theme.BubbleIn);
        Set(resources, "TgBubbleOut", theme.BubbleOut);
        Set(resources, "TgBubbleInText", theme.BubbleInText);
        Set(resources, "TgBubbleOutText", theme.BubbleOutText);
        Set(resources, "TgMetaIn", theme.MetaIn);
        Set(resources, "TgMetaOut", theme.MetaOut);
        Set(resources, "TgQuoteIn", theme.QuoteIn);
        Set(resources, "TgQuoteOut", theme.QuoteOut);
        Set(resources, "TgTick", theme.Tick);
        Set(resources, "TgServicePill", theme.ServicePill);
        Set(resources, "TgServiceText", theme.ServiceText);
        Set(resources, "TgBadge", theme.Badge);
        Set(resources, "TgBadgeMuted", theme.BadgeMuted);
        Set(resources, "TgBadgeText", theme.BadgeText);
        Set(resources, "TgOnline", theme.Online);
        Set(resources, "TgDanger", theme.Danger);
        Set(resources, "TgRowActive", theme.RowActive);
        Set(resources, "TgRowHover", theme.RowHover);
        Set(resources, "TgSelection", theme.Selection);

        if (resources.TryGetValue("TgWallpaper", out object? wallpaper)
            && wallpaper is LinearGradientBrush { GradientStops.Count: 2 } gradient)
        {
            gradient.GradientStops[0].Color = ToColor(theme.WallpaperTop);
            gradient.GradientStops[1].Color = ToColor(theme.WallpaperBottom);
        }
    }

    private static void Set(ResourceDictionary resources, string key, uint value)
    {
        if (resources.TryGetValue(key, out object? brush) && brush is SolidColorBrush solid)
        {
            solid.Color = ToColor(value);
        }
    }
}
