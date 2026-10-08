using System.ComponentModel;
using System.Text.Json;
using System.Text.Json.Serialization;
using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.UI;

namespace TuratText.Windows;

public sealed record CoreResponse(bool Ok, AppSnapshot? Snapshot, JsonElement? Value, string? Error)
{
    /// <summary>Строковое поле результата команды — например идентификатор фоновой передачи.</summary>
    public string Text(string name) =>
        Value is JsonElement value && value.ValueKind == JsonValueKind.Object
        && value.TryGetProperty(name, out JsonElement found) && found.ValueKind == JsonValueKind.String
            ? found.GetString() ?? string.Empty
            : string.Empty;

    public long Number(string name) =>
        Value is JsonElement value && value.ValueKind == JsonValueKind.Object
        && value.TryGetProperty(name, out JsonElement found) && found.TryGetInt64(out long number)
            ? number
            : 0;
}

public sealed record IdentityModel(string UserId, string DeviceId, bool IsAuthority);

public sealed record ProfileModel(string Username, string DisplayName, string About, string? AvatarBase64);

public sealed record SettingsModel(string BootstrapUrl, string MetadataProtection, bool PublishPresence);

/// <summary>Строка списка чатов: контакт плюс превью последнего события и непрочитанные.</summary>
public sealed record ChatModel(
    string UserId,
    string DisplayName,
    string? Username,
    string? About,
    string? AvatarBase64,
    bool FingerprintVerified,
    bool PendingApproval,
    long? LastSeenUnixMilliseconds,
    bool Pinned,
    bool Muted,
    string Draft,
    bool ManualUnread,
    string Preview,
    long LastActivityUnixMilliseconds,
    bool HasLastMessage,
    bool LastMessageOutgoing,
    bool LastMessageDelivered,
    bool LastMessageRead,
    int UnreadCount,
    bool IsGroup = false,
    int MemberCount = 0,
    string? GroupRole = null,
    bool GroupLeft = false,
    bool IsChannel = false,
    string? ChannelRole = null,
    bool ChannelCanPost = false)
{
    [JsonIgnore] public string Initials => Formatting.Initials(DisplayName);
    [JsonIgnore] public Brush AvatarBrush => AvatarPalette.For(UserId);
    [JsonIgnore] public string TimeLabel => Formatting.ChatListTime(LastActivityUnixMilliseconds);
    [JsonIgnore] public Visibility GroupVisibility => IsGroup ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public Visibility ChannelVisibility => IsChannel ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Писать можно: диалог принят, из группы не вышли, а в канале есть право публикации.</summary>
    [JsonIgnore] public bool CanWrite => !PendingApproval && !GroupLeft && (!IsChannel || ChannelCanPost);

    [JsonIgnore]
    public string PreviewText => Draft.Length > 0
        ? Draft
        : PendingApproval ? (IsChannel ? "Приглашение в канал" : IsGroup ? "Приглашение в группу" : "Хочет начать диалог")
        : Preview.Length > 0 ? Preview
        : IsChannel ? Formatting.Subscribers(MemberCount)
        : IsGroup ? Formatting.Members(MemberCount)
        : Username is null ? Formatting.ShortId(UserId) : "@" + Username;

    [JsonIgnore] public Visibility DraftVisibility => Draft.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public Visibility PinnedVisibility => Pinned && UnreadCount == 0 && !ManualUnread ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public Visibility MutedVisibility => Muted ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public Visibility UnreadDotVisibility => ManualUnread && UnreadCount == 0 ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public string PinMenuLabel => Pinned ? "Открепить" : "Закрепить";
    [JsonIgnore] public string MuteMenuLabel => Muted ? "Включить звук" : "Отключить звук";

    [JsonIgnore] public string UnreadLabel => UnreadCount > 999 ? "999+" : UnreadCount.ToString();
    [JsonIgnore] public Visibility UnreadVisibility => UnreadCount > 0 ? Visibility.Visible : Visibility.Collapsed;
    /// <summary>Чат «без звука» показывает приглушённый счётчик, как в Telegram.</summary>
    [JsonIgnore]
    public Brush UnreadBadgeBrush =>
        (Brush)Application.Current.Resources[Muted ? "TgBadgeMuted" : "TgBadge"];
    [JsonIgnore] public Visibility VerifiedVisibility => FingerprintVerified ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public Visibility OnlineVisibility => !IsGroup && Formatting.IsOnline(LastSeenUnixMilliseconds) ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore]
    public Visibility SingleTickVisibility =>
        HasLastMessage && LastMessageOutgoing && !LastMessageDelivered ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore]
    public Visibility DoubleTickVisibility =>
        HasLastMessage && LastMessageOutgoing && LastMessageDelivered ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore]
    public string Presence => IsChannel
        ? GroupLeft ? "вы не подписаны" : PendingApproval ? "приглашение в канал" : Formatting.Subscribers(MemberCount)
        : !IsGroup
        ? Formatting.Presence(PendingApproval, LastSeenUnixMilliseconds)
        : GroupLeft ? "вы не участник группы"
        : PendingApproval ? "приглашение в группу"
        : Formatting.Members(MemberCount);
    [JsonIgnore] public string SecurityBadge => FingerprintVerified ? "Fingerprint сверен" : "Fingerprint не сверен";
}

/// <summary>Категория вложения: от неё зависит, рисует ли пузырь превью, плеер или карточку файла.</summary>
public enum MediaKind
{
    File,
    Image,
    Video,
    Audio,
}

public sealed record AttachmentModel(
    string AttachmentId,
    string FileName,
    string MimeType,
    long Size,
    string LocalPath,
    string? Kind = null,
    int Width = 0,
    int Height = 0,
    long DurationMilliseconds = 0,
    string? ThumbnailBase64 = null)
{
    [JsonIgnore] public string SizeLabel => Formatting.Bytes(Size);

    [JsonIgnore]
    public MediaKind Media => Kind switch
    {
        "image" => MediaKind.Image,
        "video" => MediaKind.Video,
        "audio" => MediaKind.Audio,
        "file" => MediaKind.File,
        _ => MimeType.StartsWith("image/", StringComparison.OrdinalIgnoreCase) ? MediaKind.Image
            : MimeType.StartsWith("video/", StringComparison.OrdinalIgnoreCase) ? MediaKind.Video
            : MimeType.StartsWith("audio/", StringComparison.OrdinalIgnoreCase) ? MediaKind.Audio
            : MediaKind.File,
    };

    /// <summary>Превью в пузыре: ширина фиксирована, высота выводится из пропорций оригинала.</summary>
    [JsonIgnore]
    public double PreviewHeight => Width > 0 && Height > 0
        ? Math.Clamp(PreviewWidth * Height / (double)Width, 120, 420)
        : 240;

    [JsonIgnore] public double PreviewWidth => 320;

    [JsonIgnore]
    public string DurationLabel => DurationMilliseconds <= 0
        ? string.Empty
        : TimeSpan.FromMilliseconds(DurationMilliseconds) is TimeSpan span && span.TotalHours >= 1
            ? span.ToString(@"h\:mm\:ss")
            : span.ToString(@"m\:ss");
}

public sealed record MessageModel(
    string EventId,
    string SenderUserId,
    string Text,
    long CreatedAtUnixMilliseconds,
    bool Outgoing,
    bool Edited,
    bool Deleted,
    IReadOnlyList<string> Reactions,
    bool Delivered,
    bool Read,
    bool Pinned,
    AttachmentModel? Attachment,
    string? ReplyToEventId,
    string? ForwardedFrom,
    bool Service = false,
    string? SenderName = null,
    ChannelPostInfoModel? ChannelPost = null) : INotifyPropertyChanged
{
    private ImageSource? _preview;

    /// <summary>Пост канала: у всех, включая автора, рисуется слева, как в Telegram.</summary>
    [JsonIgnore] public bool AsPost { get; set; }

    /// <summary>Кнопка комментариев видна, когда они включены в канале или уже есть.</summary>
    [JsonIgnore] public bool ShowComments { get; set; }

    [JsonIgnore]
    public Visibility ChannelPostVisibility =>
        ChannelPost is not null && !Deleted ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore] public string ChannelViewsLabel => ChannelPost is null ? string.Empty : "👁 " + Formatting.Compact(ChannelPost.Views);

    [JsonIgnore]
    public string ChannelReactionsLabel => ChannelPost is null
        ? string.Empty
        : string.Join("   ", ChannelPost.Reactions.Select(value => $"{value.Reaction} {value.Count}"));

    [JsonIgnore]
    public string CommentsLabel => ChannelPost is null || ChannelPost.Comments == 0
        ? "Прокомментировать"
        : Formatting.Comments(ChannelPost.Comments);

    [JsonIgnore]
    public Visibility CommentsVisibility =>
        ShowComments || ChannelPost is { Comments: > 0 } ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Имя автора над первым пузырём подряд в группе; заполняется окном.</summary>
    [JsonIgnore] public bool ShowSender { get; set; }

    [JsonIgnore] public string SenderLabel => SenderName ?? string.Empty;
    [JsonIgnore] public Visibility SenderVisibility => ShowSender && SenderName is not null ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Цвет имени совпадает с цветом аватарки автора — так авторов легко различать.</summary>
    [JsonIgnore] public Brush SenderBrush => AvatarPalette.For(SenderUserId);

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>
    /// Превью картинки или кадра видео. Заполняется окном асинхронно: пузырь появляется сразу,
    /// а расшифровка идёт следом, поэтому лента не ждёт тяжёлые вложения.
    /// </summary>
    [JsonIgnore]
    public ImageSource? Preview
    {
        get => _preview;
        set
        {
            if (ReferenceEquals(_preview, value)) return;
            _preview = value;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(Preview)));
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(PlaceholderVisibility)));
        }
    }

    /// <summary>Пока превью не готово, на его месте видна затемнённая заглушка с индикатором.</summary>
    [JsonIgnore]
    public Visibility PlaceholderVisibility => _preview is null ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore] public MediaKind Media => Attachment?.Media ?? MediaKind.File;

    [JsonIgnore]
    public Visibility MediaVisibility =>
        Media is MediaKind.Image or MediaKind.Video ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Обычный файл показывается карточкой — превью у него всё равно нет.</summary>
    [JsonIgnore]
    public Visibility FileVisibility =>
        Attachment is not null && Media is not (MediaKind.Image or MediaKind.Video)
            ? Visibility.Visible
            : Visibility.Collapsed;

    [JsonIgnore]
    public Visibility PlayVisibility => Media == MediaKind.Video ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore] public double PreviewWidth => Attachment?.PreviewWidth ?? 320;
    [JsonIgnore] public double PreviewHeight => Attachment?.PreviewHeight ?? 240;

    /// <summary>Плашка в углу превью: длительность у видео и вес у фотографии.</summary>
    [JsonIgnore]
    public string MediaBadge => Attachment is null
        ? string.Empty
        : Media == MediaKind.Video && Attachment.DurationMilliseconds > 0
            ? Attachment.DurationLabel
            : Attachment.SizeLabel;

    /// <summary>Telegram склеивает подряд идущие сообщения одного автора в одну группу.</summary>
    [JsonIgnore] public bool FirstInGroup { get; set; } = true;

    [JsonIgnore] public bool LastInGroup { get; set; } = true;

    [JsonIgnore] public string DisplayText => Deleted ? "Сообщение удалено" : Text;
    [JsonIgnore] public Visibility TextVisibility => DisplayText.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public string TimeLabel => Formatting.Time(CreatedAtUnixMilliseconds);
    [JsonIgnore] public string ReactionSummary => string.Join(" ", Reactions);
    [JsonIgnore] public string PinMenuLabel => Pinned ? "Открепить" : "Закрепить";
    [JsonIgnore] public string EditedLabel => Edited ? "изм." : string.Empty;
    [JsonIgnore] public Visibility EditedVisibility => Edited ? Visibility.Visible : Visibility.Collapsed;
    [JsonIgnore] public Visibility AttachmentVisibility => Attachment is null ? Visibility.Collapsed : Visibility.Visible;
    [JsonIgnore] public Visibility ReactionVisibility => Reactions.Count == 0 ? Visibility.Collapsed : Visibility.Visible;

    [JsonIgnore]
    public Visibility SingleTickVisibility => Outgoing && !Delivered ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore]
    public Visibility DoubleTickVisibility => Outgoing && Delivered ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore] public Thickness RowMargin => new(0, FirstInGroup ? 8 : 2, 0, 0);

    /// <summary>Цитата ответа и заголовок пересылки заполняются окном при построении ленты.</summary>
    [JsonIgnore] public string ReplyAuthor { get; set; } = string.Empty;

    [JsonIgnore] public string ReplyText { get; set; } = string.Empty;

    [JsonIgnore] public Visibility ReplyVisibility => ReplyText.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore] public string ForwardLabel => ForwardedFrom is null ? string.Empty : "Переслано от " + ForwardedFrom;

    [JsonIgnore] public Visibility ForwardVisibility => ForwardedFrom is null ? Visibility.Collapsed : Visibility.Visible;

    [JsonIgnore] public string Quote => Deleted ? "Сообщение удалено"
        : Text.Length > 0 ? Text
        : Attachment is null ? "Сообщение"
        : Media switch
        {
            MediaKind.Image => "🖼 Фото",
            MediaKind.Video => "🎬 Видео",
            MediaKind.Audio => "🎧 " + Attachment.FileName,
            _ => "📎 " + Attachment.FileName,
        };

    /// <summary>Скруглениe как в Telegram: «хвост» у последнего пузыря группы.</summary>
    [JsonIgnore]
    public CornerRadius BubbleCorners => Outgoing
        ? new CornerRadius(14, FirstInGroup ? 14 : 5, LastInGroup ? 4 : 5, 14)
        : new CornerRadius(FirstInGroup ? 14 : 5, 14, 14, LastInGroup ? 4 : 5);
}

/// <summary>
/// Строка ленты для вложения, которое прямо сейчас шифруется перед отправкой.
/// </summary>
/// <remarks>
/// Сообщение появляется в базе только после того, как файл готов, поэтому до этого момента
/// в ленте стоит эта строка: имя файла, полоса прогресса и кнопка отмены. Без неё отправка
/// большого видео выглядела бы так, будто ничего не произошло.
/// </remarks>
public sealed class TransferModel(string jobId, string userId, string fileName, long total)
    : INotifyPropertyChanged
{
    private long _done;
    private long _total = total;
    private string _error = string.Empty;

    public event PropertyChangedEventHandler? PropertyChanged;

    public string JobId { get; } = jobId;
    public string UserId { get; } = userId;
    public string FileName { get; } = fileName;

    public long Done
    {
        get => _done;
        set => Set(ref _done, value);
    }

    public long Total
    {
        get => _total;
        set => Set(ref _total, value);
    }

    public string Error
    {
        get => _error;
        set => Set(ref _error, value);
    }

    public double Percent => _total <= 0 ? 0 : Math.Clamp(_done * 100d / _total, 0, 100);

    public string SizeLabel => $"{Formatting.Bytes(_done)} из {Formatting.Bytes(_total)}";

    public Visibility ErrorVisibility => _error.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    private void Set<T>(ref T field, T value)
    {
        if (EqualityComparer<T>.Default.Equals(field, value)) return;
        field = value;
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(null));
    }
}

/// <summary>Разделитель дня в ленте сообщений.</summary>
public sealed class DaySeparator(string label)
{
    public string Label { get; } = label;
}

public sealed record AppSnapshot(
    IdentityModel Identity,
    ProfileModel Profile,
    IReadOnlyList<ChatModel> Chats,
    string? SelectedContactId,
    IReadOnlyList<MessageModel> Messages,
    SettingsModel Settings,
    bool Online,
    string StatusMessage,
    bool OnboardingRequired,
    string SearchQuery,
    IReadOnlyList<SearchHitModel> SearchResults,
    GroupViewModel? Group = null,
    ChannelViewModel? Channel = null,
    IReadOnlyList<MessageModel>? Comments = null,
    [property: JsonPropertyName("account")] AccountModel? AccountView = null)
{
    /// <summary>Учётная запись; ядро старой версии её не присылает.</summary>
    [JsonIgnore] public AccountModel Account => AccountView ?? AccountModel.None;

    [JsonIgnore] public ChatModel? SelectedChat => Chats.FirstOrDefault(value => value.UserId == SelectedContactId);
}

/// <summary>
/// Учётная запись. <c>State</c>: <c>none</c> — нужен вход или регистрация; <c>legacy</c> —
/// переписка есть, а аккаунта ещё нет; <c>active</c> — вход выполнен.
/// </summary>
public sealed record AccountModel(
    string State,
    string Username,
    string Node,
    string? RecoveryKey,
    bool UsernameConflict,
    string? Notice,
    IReadOnlyList<AccountDeviceModel> Devices)
{
    public static readonly AccountModel None = new("none", string.Empty, string.Empty, null, false, null, []);
}

public sealed record AccountDeviceModel(string DeviceId, string Name, bool Current, long AddedAtUnixMilliseconds);

public sealed record GroupPermissionsModel(bool MembersCanInvite, bool MembersCanEditInfo);

/// <summary>Участник в карточке группы. Имя уже разрешено ядром через контакты.</summary>
public sealed record GroupMemberModel(
    string UserId,
    string DisplayName,
    string? AvatarBase64,
    string Role,
    bool IsSelf,
    bool IsContact,
    bool Confirmed,
    string AddedByName)
{
    [JsonIgnore] public string Initials => Formatting.Initials(DisplayName);
    [JsonIgnore] public Brush AvatarBrush => AvatarPalette.For(UserId);

    [JsonIgnore]
    public string RoleLabel => Role switch
    {
        "owner" => "владелец",
        "admin" => "админ",
        _ => string.Empty,
    };

    [JsonIgnore] public Visibility RoleVisibility => RoleLabel.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

    [JsonIgnore]
    public string Subtitle => IsSelf ? "это вы"
        : !Confirmed ? "ещё не подтвердил(а) участие"
        : IsContact ? "в ваших контактах"
        : "добавил(а) " + AddedByName;

    /// <summary>Кнопка действий видна, только если текущему пользователю есть что предложить.</summary>
    [JsonIgnore] public Visibility MenuVisibility { get; set; } = Visibility.Collapsed;

    public static int Rank(string? role) => role switch
    {
        "owner" => 2,
        "admin" => 1,
        "member" => 0,
        _ => -1,
    };
}

/// <summary>Карточка открытой группы и права текущего пользователя в ней — их считает ядро.</summary>
public sealed record GroupViewModel(
    string GroupId,
    string Name,
    string About,
    string? AvatarBase64,
    long Epoch,
    string CreatedBy,
    long CreatedAtUnixMilliseconds,
    string? MyRole,
    bool PendingInvite,
    string? InvitedByName,
    bool Left,
    GroupPermissionsModel Permissions,
    IReadOnlyList<GroupMemberModel> Members,
    bool CanSend,
    bool CanInvite,
    bool CanEditInfo,
    bool CanRemoveMembers,
    bool CanManageAdmins,
    bool CanDeleteMessages);

public sealed record ReactionCountModel(string Reaction, int Count, bool Mine);

/// <summary>Просмотры, комментарии и реакции поста канала.</summary>
public sealed record ChannelPostInfoModel(int Views, int Comments, IReadOnlyList<ReactionCountModel> Reactions);

/// <summary>Права администратора канала — те же семь, что в Telegram.</summary>
public sealed record ChannelRightsModel(
    bool PostMessages = false,
    bool EditMessages = false,
    bool DeleteMessages = false,
    bool InviteUsers = false,
    bool ChangeInfo = false,
    bool BanUsers = false,
    bool AddAdmins = false)
{
    [JsonIgnore]
    public string Summary
    {
        get
        {
            var parts = new List<string>();
            if (PostMessages) parts.Add("публикует");
            if (EditMessages) parts.Add("правит");
            if (DeleteMessages) parts.Add("удаляет");
            if (InviteUsers) parts.Add("приглашает");
            if (ChangeInfo) parts.Add("меняет данные");
            if (BanUsers) parts.Add("блокирует");
            if (AddAdmins) parts.Add("назначает админов");
            return parts.Count == 0 ? "без прав" : string.Join(", ", parts);
        }
    }
}

public sealed record ChannelSettingsModel(
    bool SignPosts,
    bool CommentsEnabled,
    string? DiscussionGroupId,
    string DiscussionGroupName);

public sealed record ChannelAdminModel(
    string UserId,
    string DisplayName,
    string? AvatarBase64,
    string Role,
    ChannelRightsModel Rights,
    string Title,
    bool IsSelf,
    string AddedByName,
    bool CanEdit);

public sealed record ChannelSubscriberModel(
    string UserId,
    string DisplayName,
    string? AvatarBase64,
    bool IsContact,
    bool Banned,
    long SubscribedAtUnixMilliseconds);

/// <summary>Карточка открытого канала и права текущего пользователя в нём — их считает ядро.</summary>
public sealed record ChannelViewModel(
    string ChannelId,
    string Name,
    string About,
    string? AvatarBase64,
    long Epoch,
    string? MyRole,
    ChannelRightsModel MyRights,
    bool AwaitingState,
    bool PendingInvite,
    string? InvitedByName,
    bool Left,
    bool Removed,
    bool Closed,
    ChannelSettingsModel Settings,
    bool DiscussionJoined,
    int SubscriberCount,
    IReadOnlyList<ChannelAdminModel> Admins,
    IReadOnlyList<ChannelSubscriberModel> Subscribers,
    string? InviteLink,
    bool CanPost,
    bool CanEditInfo,
    bool CanInvite,
    bool CanBan,
    bool CanAddAdmins,
    bool CanDeleteMessages,
    bool CanEditMessages,
    bool CanComment,
    bool CanReact,
    string? ThreadPostEventId)
{
    [JsonIgnore] public bool Active => !AwaitingState && !PendingInvite && !Left && !Removed && !Closed;

    [JsonIgnore]
    public string StateLabel => Closed ? "канал удалён"
        : Removed ? "вас удалили из канала"
        : AwaitingState ? "ждём ответа администратора"
        : PendingInvite ? "приглашение в канал"
        : Left ? "вы отписались"
        : Formatting.Subscribers(SubscriberCount);
}

/// <summary>Найденное сообщение в глобальном поиске.</summary>
public sealed record SearchHitModel(
    string EventId,
    string UserId,
    string DisplayName,
    string? AvatarBase64,
    string Text,
    long CreatedAtUnixMilliseconds,
    bool Outgoing)
{
    [JsonIgnore] public string Initials => Formatting.Initials(DisplayName);
    [JsonIgnore] public Brush AvatarBrush => AvatarPalette.For(UserId);
    [JsonIgnore] public string TimeLabel => Formatting.ChatListTime(CreatedAtUnixMilliseconds);
    [JsonIgnore] public string Preview => (Outgoing ? "Вы: " : string.Empty) + Text;
}

/// <summary>Семь фирменных градиентов аватарок Telegram.</summary>
internal static class AvatarPalette
{
    private static readonly (uint Top, uint Bottom)[] Gradients =
    [
        (0xFFFF885E, 0xFFFF516A),
        (0xFFFFCD6A, 0xFFFFA85C),
        (0xFFE0A2F3, 0xFFD669ED),
        (0xFFA0DE7E, 0xFF54CB68),
        (0xFF53EDD6, 0xFF28C9B7),
        (0xFF72D5FD, 0xFF2A9EF1),
        (0xFFB694F9, 0xFF6C61DF),
    ];

    public static Brush For(string key)
    {
        (uint top, uint bottom) = Gradients[(int)(Formatting.StableHash(key) % (uint)Gradients.Length)];
        var brush = new LinearGradientBrush { StartPoint = new(0, 0), EndPoint = new(0, 1) };
        brush.GradientStops.Add(new GradientStop { Color = FromArgb(top), Offset = 0 });
        brush.GradientStops.Add(new GradientStop { Color = FromArgb(bottom), Offset = 1 });
        return brush;
    }

    private static Color FromArgb(uint value) =>
        ColorHelper.FromArgb((byte)(value >> 24), (byte)(value >> 16), (byte)(value >> 8), (byte)value);
}

internal static class Formatting
{
    private static readonly string[] Months =
    [
        "января", "февраля", "марта", "апреля", "мая", "июня",
        "июля", "августа", "сентября", "октября", "ноября", "декабря",
    ];

    private static readonly string[] Weekdays = ["вс", "пн", "вт", "ср", "чт", "пт", "сб"];

    /// <summary>Совпадает с String.hashCode() из Java, чтобы аватарки были одного цвета на всех клиентах.</summary>
    public static uint StableHash(string value)
    {
        unchecked
        {
            int hash = 0;
            foreach (char symbol in value) hash = (31 * hash) + symbol;
            return (uint)Math.Abs((long)hash);
        }
    }

    public static string Initials(string name)
    {
        string[] words = name.Trim().Split(' ', StringSplitOptions.RemoveEmptyEntries);
        return words.Length switch
        {
            0 => "#",
            1 => words[0][..1].ToUpperInvariant(),
            _ => (words[0][..1] + words[^1][..1]).ToUpperInvariant(),
        };
    }

    public static string ShortId(string value) => value.Length <= 20 ? value : value[..11] + "…" + value[^6..];

    public static string Bytes(long value) => value < 1024
        ? $"{value} Б"
        : value < 1024 * 1024 ? $"{value / 1024} КБ" : $"{value / 1048576d:F1} МБ";

    public static DateTimeOffset Local(long value) => DateTimeOffset.FromUnixTimeMilliseconds(value).ToLocalTime();

    public static string Time(long value) => Local(value).ToString("HH:mm");

    private static int DaysFromToday(long value) =>
        (DateTime.Today - Local(value).Date).Days;

    public static string ChatListTime(long value)
    {
        if (value <= 0) return string.Empty;
        int days = DaysFromToday(value);
        DateTimeOffset moment = Local(value);
        return days switch
        {
            <= 0 => moment.ToString("HH:mm"),
            < 7 => Weekdays[(int)moment.DayOfWeek],
            < 330 => moment.ToString("dd.MM"),
            _ => moment.ToString("dd.MM.yy"),
        };
    }

    public static string DateSeparator(long value)
    {
        int days = DaysFromToday(value);
        DateTimeOffset moment = Local(value);
        return days switch
        {
            0 => "Сегодня",
            1 => "Вчера",
            < 330 => $"{moment.Day} {Months[moment.Month - 1]}",
            _ => $"{moment.Day} {Months[moment.Month - 1]} {moment.Year}",
        };
    }

    public static bool SameDay(long left, long right) => Local(left).Date == Local(right).Date;

    /// <summary>«1 участник», «3 участника», «11 участников».</summary>
    public static string Members(int count)
    {
        int tens = count % 100;
        int units = count % 10;
        string word = tens is >= 11 and <= 14 ? "участников"
            : units == 1 ? "участник"
            : units is >= 2 and <= 4 ? "участника"
            : "участников";
        return $"{count} {word}";
    }

    /// <summary>«1 подписчик», «3 подписчика», «11 подписчиков».</summary>
    public static string Subscribers(int count)
    {
        int tens = count % 100;
        int units = count % 10;
        string word = tens is >= 11 and <= 14 ? "подписчиков"
            : units == 1 ? "подписчик"
            : units is >= 2 and <= 4 ? "подписчика"
            : "подписчиков";
        return $"{count} {word}";
    }

    /// <summary>«1 комментарий», «2 комментария», «5 комментариев».</summary>
    public static string Comments(int count)
    {
        int tens = count % 100;
        int units = count % 10;
        string word = tens is >= 11 and <= 14 ? "комментариев"
            : units == 1 ? "комментарий"
            : units is >= 2 and <= 4 ? "комментария"
            : "комментариев";
        return $"{count} {word}";
    }

    /// <summary>Компактное число просмотров, как в Telegram: 950, 1,2K, 34K.</summary>
    public static string Compact(int value) => value switch
    {
        >= 1_000_000 => (value / 1_000_000d).ToString("0.#") + "M",
        >= 10_000 => (value / 1000) + "K",
        >= 1_000 => (value / 1000d).ToString("0.#") + "K",
        _ => value.ToString(),
    };

    public static bool IsOnline(long? lastSeen) =>
        lastSeen is long value && DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() - value < 90_000;

    public static string Presence(bool pending, long? lastSeen)
    {
        if (pending) return "запрос на общение";
        if (lastSeen is not long value) return "был(а) недавно";
        if (IsOnline(lastSeen)) return "в сети";
        return DaysFromToday(value) switch
        {
            0 => $"был(а) в {Time(value)}",
            1 => $"был(а) вчера в {Time(value)}",
            _ => $"был(а) {DateSeparator(value)}",
        };
    }
}
