using System.Collections.ObjectModel;
using System.Text;
using Microsoft.UI;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using TuratText.Windows.Interop;
using Windows.ApplicationModel.DataTransfer;
using Windows.Graphics;
using Windows.Graphics.Imaging;
using Windows.Storage;
using Windows.Storage.Pickers;
using Windows.Storage.Streams;
using Windows.System;
using Windows.UI.Core;
using WinRT.Interop;

namespace TuratText.Windows;

public sealed partial class MainWindow : Window
{
    private static readonly string[] ReactionSet = ["❤", "🔥", "👌", "😱", "😭", "🤨", "👍", "💔"];
    private static readonly FontFamily IconFontFamily = new("Segoe MDL2 Assets");

    /// <summary>Запасной интервал опроса и частые повторы, пока связи нет.</summary>
    private static readonly TimeSpan OnlineSyncInterval = TimeSpan.FromSeconds(20);
    private static readonly TimeSpan OfflineRetryInterval = TimeSpan.FromSeconds(6);
    /// <summary>Окно ожидания конверта; Node ограничивает его своей настройкой.</summary>
    private const int WaitWindowSeconds = 25;

    private readonly RustCore _core;
    private readonly ObservableCollection<object> _sidebar = [];
    private readonly ObservableCollection<object> _feed = [];

    /// <summary>Отпечаток уже нарисованной ленты; <c>null</c> — лента пуста.</summary>
    private string? _feedSignature;

    /// <summary>Разделитель полей в отпечатке: в тексте сообщения такого символа быть не может.</summary>
    private const char FieldSeparator = (char)31;
    private readonly ObservableCollection<ChatModel> _forwardTargets = [];
    private readonly DispatcherTimer _searchTimer = new() { Interval = TimeSpan.FromMilliseconds(280) };
    private readonly ObservableCollection<ThemePalette> _themes = [.. ThemeCatalog.All];
    private readonly ObservableCollection<AppFontChoice> _fonts = [.. FontCatalog.All];
    private ThemePalette _theme = ThemeCatalog.All[0];
    private AppFontChoice _font = FontCatalog.Default;
    private bool _syncing;
    private bool _watching;
    private bool _closing;
    private bool _submitting;
    private AppSnapshot? _snapshot;
    private bool _updating;
    private bool _onboardingShown;
    private string? _editingEventId;
    private string? _replyToEventId;
    private string? _draftChatId;
    private string[] _forwardEventIds = [];
    private bool _messageSelectionMode;

    public MainWindow()
    {
        InitializeComponent();
        _core = new RustCore();
        // Запасной путь на случай, если PreviewKeyDown не дойдёт: обычный KeyDown из разметки
        // не вызывается для клавиш, которые TextBox уже обработал сам.
        ComposerInput.AddHandler(
            UIElement.KeyDownEvent, new KeyEventHandler(ComposerInput_KeyDown), handledEventsToo: true);
        ChatsList.ItemsSource = _sidebar;
        MessagesList.ItemsSource = _feed;
        ForwardList.ItemsSource = _forwardTargets;
        ThemeList.ItemsSource = _themes;
        FontList.ItemsSource = _fonts;
        _searchTimer.Tick += SearchTimer_Tick;

        ExtendsContentIntoTitleBar = true;
        SetTitleBar(TitleBar);
        // Liquid Glass: за полупрозрачными панелями приложения лежит системный acrylic, поэтому
        // сквозь них видно и размытый рабочий стол, и слои самого окна.
        try
        {
            SystemBackdrop = new DesktopAcrylicBackdrop();
        }
        catch
        {
            // На сборках без поддержки backdrop окно останется с обычной заливкой темы.
        }
        try
        {
            AppWindow.Resize(new SizeInt32(1240, 820));
        }
        catch
        {
            // Размер окна — необязательная деталь запуска.
        }

        Closed += (_, _) =>
        {
            _closing = true;
            _searchTimer.Stop();
            StopUpdates();
            PersistDraft();
            CloseMediaViewer();
            _core.Dispose();
        };
    }

    private async void Root_Loaded(object sender, RoutedEventArgs e)
    {
        ApplyTheme(ThemeCatalog.Resolve(UiSettings.ThemeId));
        ApplyFont(FontCatalog.Resolve(UiSettings.FontId));
        StartUpdateChecks();
        await ExecuteAsync(new { command = "snapshot" });
        // Клиент подключается к Node сам: кнопка синхронизации — ускоритель, а не условие связи.
        await SyncAsync();
        ScheduleNextSync();
    }

    // --- автоматическая связь с Node -----------------------------------------

    /// <summary>
    /// Запускает фоновый цикл связи, если он ещё не идёт. Вызов повторно безопасен: кнопки
    /// «Синхронизировать» и «Подключиться» только убеждаются, что цикл жив.
    /// </summary>
    private void ScheduleNextSync()
    {
        if (_watching || _closing) return;
        _watching = true;
        _ = WatchLoopAsync();
    }

    /// <summary>
    /// Node держит запрос на входящие открытым и отвечает в тот момент, когда конверт приходит,
    /// поэтому сообщение попадает в ленту за доли секунды, а не к следующему циклу опроса.
    /// Опрос по таймеру остаётся запасным путём: пока связи нет и пока Node не умеет ждать.
    /// </summary>
    private async Task WatchLoopAsync()
    {
        try
        {
            while (!_closing)
            {
                if (_snapshot?.Online != true)
                {
                    await Task.Delay(OfflineRetryInterval);
                }
                else
                {
                    // Ожидание идёт мимо ядра: отправка сообщения его не ждёт.
                    int awaited = await Task.Run(() => _core.WaitForEnvelopes(WaitWindowSeconds));
                    // Конверт пришёл или окно истекло — ниже цикл сам за ним сходит.
                    // Ждать негде (-1) — возвращаемся к прежнему интервалу опроса.
                    if (awaited < 0) await Task.Delay(OnlineSyncInterval);
                }
                if (_closing) return;
                await SyncAsync();
            }
        }
        finally
        {
            _watching = false;
        }
    }

    /// <summary>
    /// Фоновый цикл: без индикатора занятости и без диалогов — обрыв связи виден по подписи
    /// состояния, а не по всплывающему окну каждые несколько секунд.
    /// </summary>
    private async Task SyncAsync()
    {
        if (_syncing) return;
        _syncing = true;
        try
        {
            CoreResponse result = await Task.Run(() => _core.Invoke(new { command = "sync" }));
            if (result.Snapshot is not null) ApplySnapshot(result.Snapshot);
            await MarkOpenChatReadAsync();
        }
        catch (Exception exception)
        {
            StatusText.Text = exception.Message;
        }
        finally
        {
            _syncing = false;
        }
    }

    /// <summary>
    /// Отмечает прочитанным диалог, который сейчас открыт.
    /// </summary>
    /// <remarks>
    /// Открытый диалог виден пользователю целиком, поэтому пришедшее в него сообщение прочитано
    /// в тот же момент. Раньше отметка ставилась только при выборе чата, и переписка в уже
    /// открытом окне так и оставалась непрочитанной.
    /// </remarks>
    private async Task MarkOpenChatReadAsync()
    {
        if (_snapshot?.SelectedChat is not { UnreadCount: > 0 } chat) return;
        CoreResponse read = await Task.Run(() => _core.Invoke(new
        {
            command = "mark_read",
            user_id = chat.UserId,
        }));
        if (read.Snapshot is not null) ApplySnapshot(read.Snapshot);
    }

    // --- ядро -----------------------------------------------------------------

    private async Task<bool> ExecuteAsync(object command, bool showErrorDialog = true)
    {
        BusyIndicator.Visibility = Visibility.Visible;
        try
        {
            CoreResponse result = await Task.Run(() => _core.Invoke(command));
            if (result.Snapshot is not null) ApplySnapshot(result.Snapshot);
            if (!result.Ok && showErrorDialog && !string.IsNullOrWhiteSpace(result.Error))
            {
                await TryShowErrorAsync(result.Error);
            }
            return result.Ok;
        }
        catch (Exception exception)
        {
            StatusText.Text = exception.Message;
            if (showErrorDialog) await TryShowErrorAsync(exception.Message);
            return false;
        }
        finally
        {
            BusyIndicator.Visibility = Visibility.Collapsed;
        }
    }

    private void ApplySnapshot(AppSnapshot snapshot)
    {
        _updating = true;
        try
        {
            _snapshot = snapshot;
            StatusText.Text = snapshot.StatusMessage;
            ConnectionState.Text = snapshot.Online ? "в сети" : "нет связи с Node";
            ConnectionState.Foreground = (Brush)Application.Current.Resources[
                snapshot.Online ? "TgOnline" : "TgHint"];
            ToolTipService.SetToolTip(ConnectionState, snapshot.StatusMessage);
            MyInitials.Text = Formatting.Initials(
                snapshot.Profile.DisplayName.Length > 0 ? snapshot.Profile.DisplayName : "Turat");
            MyAvatar.Source = Images.Decode(snapshot.Profile.AvatarBase64);

            RebuildSidebar();
            RebuildConversation();

            if (SettingsPage.Visibility == Visibility.Visible) FillSettings();
            if (ProfilePage.Visibility == Visibility.Visible) FillProfile();

            if (snapshot.OnboardingRequired && !_onboardingShown)
            {
                _onboardingShown = true;
                _ = ShowOnboardingAsync();
            }
        }
        finally
        {
            _updating = false;
        }

        // Шаблоны строк создаются после наполнения списков. Применяем гарнитуру на следующем
        // кадре, когда новые TextBlock уже появились в visual tree.
        DispatcherQueue.TryEnqueue(() => ApplyFontToTree(Root, _font.Family));
    }

    // --- список чатов ---------------------------------------------------------

    private void RebuildSidebar()
    {
        if (_snapshot is null) return;
        string query = SearchInput.Text.Trim();
        bool searching = query.Length > 0;
        IEnumerable<ChatModel> chats = _snapshot.Chats;
        if (searching)
        {
            chats = chats.Where(chat =>
                chat.DisplayName.Contains(query, StringComparison.CurrentCultureIgnoreCase)
                || (chat.Username?.Contains(query, StringComparison.OrdinalIgnoreCase) ?? false)
                || chat.Preview.Contains(query, StringComparison.CurrentCultureIgnoreCase));
        }

        _sidebar.Clear();
        List<ChatModel> matched = [.. chats];
        if (searching && matched.Count > 0) _sidebar.Add(new SectionHeader("Чаты"));
        foreach (ChatModel chat in matched) _sidebar.Add(chat);
        if (searching && _snapshot.SearchResults.Count > 0)
        {
            _sidebar.Add(new SectionHeader("Сообщения"));
            foreach (SearchHitModel hit in _snapshot.SearchResults) _sidebar.Add(hit);
        }

        EmptyChats.Text = searching ? "Ничего не найдено" : "Здесь появятся ваши чаты";
        EmptyChats.Visibility = _sidebar.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        ClearSearchButton.Visibility = searching ? Visibility.Visible : Visibility.Collapsed;
        ChatsList.SelectedItem = _sidebar.OfType<ChatModel>()
            .FirstOrDefault(chat => chat.UserId == _snapshot.SelectedContactId);
    }

    private void ChatsList_ContainerContentChanging(ListViewBase sender, ContainerContentChangingEventArgs args)
    {
        if (args.ItemContainer is null) return;
        bool header = args.Item is SectionHeader;
        args.ItemContainer.IsHitTestVisible = !header;
        args.ItemContainer.IsEnabled = !header;
    }

    private async void ChatsList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_updating) return;
        string? userId = ChatsList.SelectedItem switch
        {
            ChatModel chat => chat.UserId,
            SearchHitModel hit => hit.UserId,
            _ => null,
        };
        if (userId is null) return;
        PersistDraft();
        await OpenChatAsync(userId);
    }

    private async Task OpenChatAsync(string userId)
    {
        if (_messageSelectionMode) ExitMessageSelectionMode();
        await ExecuteAsync(new { command = "select_contact", user_id = userId });
        await ExecuteAsync(new { command = "mark_read", user_id = userId });
    }

    private void SearchInput_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (_updating) return;
        RebuildSidebar();
        _searchTimer.Stop();
        _searchTimer.Start();
    }

    private async void SearchTimer_Tick(object? sender, object e)
    {
        _searchTimer.Stop();
        string query = SearchInput.Text.Trim();
        await ExecuteAsync(new { command = "search", query = query.Length >= 2 ? query : string.Empty });
    }

    private void ClearSearch_Click(object sender, RoutedEventArgs e) => SearchInput.Text = string.Empty;

    /// <summary>Контекстное меню чата: закрепить, звук, непрочитано, очистить, удалить.</summary>
    private void Chat_ContextRequested(UIElement sender, ContextRequestedEventArgs args)
    {
        if ((sender as FrameworkElement)?.DataContext is not ChatModel chat) return;
        var menu = new MenuFlyout();
        menu.Items.Add(MenuItem(chat.PinMenuLabel, "", async () =>
            await ExecuteAsync(new { command = "set_chat_pinned", user_id = chat.UserId, pinned = !chat.Pinned })));
        menu.Items.Add(MenuItem(chat.MuteMenuLabel, "", async () =>
            await ExecuteAsync(new { command = "set_chat_muted", user_id = chat.UserId, muted = !chat.Muted })));
        menu.Items.Add(MenuItem("Отметить непрочитанным", "", async () =>
            await ExecuteAsync(new { command = "mark_unread", user_id = chat.UserId })));
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(MenuItem("Очистить историю", "", async () =>
        {
            if (await ConfirmAsync("Очистить историю?", "Локальные сообщения этого диалога будут удалены."))
                await ExecuteAsync(new { command = "clear_history", user_id = chat.UserId });
        }));
        menu.Items.Add(MenuItem("Удалить чат", "", async () =>
        {
            if (await ConfirmAsync("Удалить диалог?", "Локальная история этого диалога будет удалена."))
                await ExecuteAsync(new { command = "delete_contact", user_id = chat.UserId });
        }));
        ShowMenu(menu, sender, args);
    }

    // --- лента сообщений ------------------------------------------------------

    private void RebuildConversation()
    {
        if (_snapshot is null) return;
        ChatModel? chat = _snapshot.SelectedChat;
        bool hasChat = chat is not null;

        ConversationHeader.Visibility = hasChat && MessagesList.SelectedItems.Count == 0
            ? Visibility.Visible
            : Visibility.Collapsed;
        MessagesList.Visibility = hasChat ? Visibility.Visible : Visibility.Collapsed;
        EmptyConversation.Visibility = hasChat ? Visibility.Collapsed : Visibility.Visible;
        Composer.Visibility = hasChat && chat!.PendingApproval == false ? Visibility.Visible : Visibility.Collapsed;
        PendingBar.Visibility = hasChat && chat!.PendingApproval ? Visibility.Visible : Visibility.Collapsed;

        if (chat is null)
        {
            _feed.Clear();
            _feedSignature = null;
            return;
        }

        ContactTitle.Text = chat.DisplayName;
        ContactInitial.Text = chat.Initials;
        ContactAvatar.Source = Images.Decode(chat.AvatarBase64);
        ContactStatus.Text = chat.Presence;
        ContactOnline.Visibility = chat.OnlineVisibility;
        ContactMuted.Visibility = chat.MutedVisibility;

        bool switchedChat = _draftChatId != chat.UserId;
        if (switchedChat)
        {
            _draftChatId = chat.UserId;
            _editingEventId = null;
            _replyToEventId = null;
            ComposerInput.Text = chat.Draft;
            UpdateBanner();
        }

        // Перестроение ленты стоит дорого и заметно: Clear() гасит ObservableCollection целиком,
        // ListView выбрасывает контейнеры и уезжает в начало переписки. Фоновая синхронизация
        // идёт каждые полминуты, поэтому лента трогается только тогда, когда правда изменилась.
        string signature = FeedSignature(chat, _snapshot.Messages);
        if (signature == _feedSignature) return;
        _feedSignature = signature;

        // Выделение сообщений и место чтения не должны пропадать при перестроении.
        HashSet<string> selectedIds = [.. SelectedMessages().Select(value => value.EventId)];
        string? lastEventId = _feed.OfType<MessageModel>().LastOrDefault()?.EventId;
        bool atBottom = IsScrolledToBottom();
        double previousOffset = FindScrollViewer(MessagesList)?.VerticalOffset ?? 0;

        _feed.Clear();
        IReadOnlyList<MessageModel> messages = _snapshot.Messages;
        Dictionary<string, MessageModel> byId = messages.ToDictionary(value => value.EventId);
        MessageModel? previous = null;
        for (int index = 0; index < messages.Count; index++)
        {
            MessageModel message = messages[index];
            bool newDay = previous is null
                          || !Formatting.SameDay(previous.CreatedAtUnixMilliseconds, message.CreatedAtUnixMilliseconds);
            if (newDay) _feed.Add(new DaySeparator(Formatting.DateSeparator(message.CreatedAtUnixMilliseconds)));
            MessageModel? next = index + 1 < messages.Count ? messages[index + 1] : null;
            message.FirstInGroup = newDay || !Grouped(previous, message) || message.ReplyToEventId is not null;
            message.LastInGroup = next is null || !Grouped(message, next) || next.ReplyToEventId is not null;
            if (message.ReplyToEventId is string replyId && byId.TryGetValue(replyId, out MessageModel? replied))
            {
                message.ReplyAuthor = replied.Outgoing ? "Вы" : chat.DisplayName;
                message.ReplyText = replied.Quote;
            }
            _feed.Add(message);
            previous = message;
        }

        foreach (TransferModel transfer in _transfers.Values.Where(value => value.UserId == chat.UserId))
        {
            _feed.Add(transfer);
        }

        // Превью грузятся после того, как лента уже на экране: пузыри не ждут расшифровки.
        LoadPreviews();

        if (selectedIds.Count > 0)
        {
            foreach (MessageModel message in _feed.OfType<MessageModel>()
                         .Where(value => selectedIds.Contains(value.EventId)))
            {
                MessagesList.SelectedItems.Add(message);
            }
        }

        // Пустая прошлая лента или другой собеседник — диалог показываем с конца.
        if (_feed.Count > 0 && (switchedChat || lastEventId is null || atBottom))
        {
            MessagesList.ScrollIntoView(_feed[^1]);
        }
        else if (previousOffset > 0)
        {
            // Читающего историю возвращаем туда, где он был: Clear() уже сбросил прокрутку
            // в начало, и без этого чтение прерывалось бы на каждой перестройке.
            RestoreScrollOffset(previousOffset);
        }
    }

    /// <summary>
    /// Отпечаток ленты: всё, от чего зависит нарисованное. Сравнение по нему дешевле
    /// перестроения и, в отличие от равенства записей, не спотыкается о список реакций —
    /// он сравнивался бы по ссылке и всегда расходился.
    /// </summary>
    private static string FeedSignature(ChatModel chat, IReadOnlyList<MessageModel> messages)
    {
        var builder = new StringBuilder(messages.Count * 48);
        builder.Append(chat.UserId).Append('|').Append(chat.DisplayName).AppendLine();
        foreach (MessageModel message in messages)
        {
            builder.Append(message.EventId).Append(FieldSeparator)
                .Append(message.Text).Append(FieldSeparator)
                .Append(message.CreatedAtUnixMilliseconds).Append(FieldSeparator)
                .Append(message.Edited ? '1' : '0')
                .Append(message.Deleted ? '1' : '0')
                .Append(message.Delivered ? '1' : '0')
                .Append(message.Read ? '1' : '0')
                .Append(message.Pinned ? '1' : '0').Append(FieldSeparator)
                .Append(string.Join(',', message.Reactions)).Append(FieldSeparator)
                .Append(message.ReplyToEventId).Append(FieldSeparator)
                .Append(message.ForwardedFrom).Append(FieldSeparator)
                .Append(message.Attachment?.AttachmentId).Append(FieldSeparator)
                .Append(message.Attachment?.Size ?? 0).AppendLine();
        }
        return builder.ToString();
    }

    /// <summary>
    /// Возвращает прокрутку на прежнее место. Сразу после наполнения коллекции ListView ещё не
    /// разложил элементы и не знает своей высоты, поэтому восстановление уходит в следующий
    /// проход очереди — к нему размеры уже посчитаны.
    /// </summary>
    private void RestoreScrollOffset(double offset)
    {
        DispatcherQueue.TryEnqueue(() =>
            FindScrollViewer(MessagesList)?.ChangeView(null, offset, null, disableAnimation: true));
    }

    /// <summary>Лента доскроллена до конца — значит новое сообщение можно показать сразу.</summary>
    private bool IsScrolledToBottom()
    {
        if (_feed.Count == 0) return true;
        ScrollViewer? scroll = FindScrollViewer(MessagesList);
        return scroll is null || scroll.ScrollableHeight - scroll.VerticalOffset < 80;
    }

    private static ScrollViewer? FindScrollViewer(DependencyObject root)
    {
        int count = VisualTreeHelper.GetChildrenCount(root);
        for (int index = 0; index < count; index++)
        {
            DependencyObject child = VisualTreeHelper.GetChild(root, index);
            if (child is ScrollViewer viewer) return viewer;
            if (FindScrollViewer(child) is ScrollViewer nested) return nested;
        }
        return null;
    }

    /// <summary>Подряд идущие сообщения одного автора склеиваются в группу.</summary>
    private static bool Grouped(MessageModel? left, MessageModel right) =>
        left is not null
        && left.Outgoing == right.Outgoing
        && left.ForwardedFrom == right.ForwardedFrom
        && Formatting.SameDay(left.CreatedAtUnixMilliseconds, right.CreatedAtUnixMilliseconds)
        && Math.Abs(right.CreatedAtUnixMilliseconds - left.CreatedAtUnixMilliseconds) < 600_000;

    private void MessagesList_ContainerContentChanging(ListViewBase sender, ContainerContentChangingEventArgs args)
    {
        if (args.ItemContainer is null) return;
        bool separator = args.Item is DaySeparator;
        args.ItemContainer.IsHitTestVisible = !separator;
        args.ItemContainer.IsEnabled = !separator;
        args.ItemContainer.ContextRequested -= Message_ContextRequested;
        if (!separator) args.ItemContainer.ContextRequested += Message_ContextRequested;
    }

    private void MessagesList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_updating) return;
        int count = SelectedMessages().Count;
        SelectionCount.Text = count.ToString();
        SelectionBar.Visibility = count > 0 ? Visibility.Visible : Visibility.Collapsed;
        ConversationHeader.Visibility = count > 0 || _snapshot?.SelectedChat is null
            ? Visibility.Collapsed
            : Visibility.Visible;
        EditSelectedButton.IsEnabled = count == 1 && SelectedMessages()[0] is { Outgoing: true, Deleted: false };
        if (count == 0 && _messageSelectionMode) ExitMessageSelectionMode();
    }

    private List<MessageModel> SelectedMessages() =>
        [.. MessagesList.SelectedItems.OfType<MessageModel>()];

    /// <summary>Карточка действий по ПКМ; в режиме мультивыделения её заменяет верхняя панель.</summary>
    private void Message_ContextRequested(UIElement sender, ContextRequestedEventArgs args)
    {
        MessageModel? message = sender switch
        {
            ListViewItem { Content: MessageModel item } => item,
            FrameworkElement { DataContext: MessageModel item } => item,
            _ => null,
        };
        if (message is null) return;
        if (_messageSelectionMode)
        {
            args.Handled = true;
            return;
        }
        var menu = new MenuFlyout();
        menu.MenuFlyoutPresenterStyle = (Style)Application.Current.Resources["TgMessageMenuPresenter"];
        if (!message.Deleted)
        {
            menu.Items.Add(MessageMenuItem("Ответить", "", () =>
            {
                _replyToEventId = message.EventId;
                _editingEventId = null;
                UpdateBanner();
                ComposerInput.Focus(FocusState.Programmatic);
            }));
        }
        if (message is { Outgoing: true, Deleted: false })
        {
            menu.Items.Add(MessageMenuItem("Изменить", "", () => BeginEdit(message)));
        }
        if (!message.Deleted)
        {
            menu.Items.Add(MessageMenuItem(message.PinMenuLabel, "", async () =>
                await ExecuteAsync(new
                {
                    command = "set_message_pinned",
                    event_id = message.EventId,
                    pinned = !message.Pinned,
                })));
            menu.Items.Add(MessageMenuItem("Переслать", "", () => ShowForwardDialog([message.EventId])));
        }
        if (message is { Outgoing: true, Deleted: false })
        {
            menu.Items.Add(MessageMenuItem("Удалить", "", async () =>
                await ExecuteAsync(new { command = "delete_messages", event_ids = new[] { message.EventId } })));
        }
        menu.Items.Add(MessageMenuItem("Выделить", "", () => EnterMessageSelectionMode(message)));
        ShowMenu(menu, sender, args);
    }

    // --- поле ввода -----------------------------------------------------------

    private void ComposerInput_TextChanged(object sender, TextChangedEventArgs e) =>
        SendButton.IsEnabled = ComposerInput.Text.Trim().Length > 0;

    /// <summary>
    /// Enter отправляет сообщение, Shift+Enter переносит строку, Esc снимает правку и ответ.
    /// Многострочный TextBox сам обрабатывает Enter и помечает событие обработанным, поэтому
    /// обычного KeyDown из разметки недостаточно: клавиша ловится в туннельном PreviewKeyDown,
    /// а KeyDown подписан с handledEventsToo как запасной путь.
    /// </summary>
    private async void ComposerInput_PreviewKeyDown(object sender, KeyRoutedEventArgs e) =>
        await HandleComposerKeyAsync(e);

    private async void ComposerInput_KeyDown(object sender, KeyRoutedEventArgs e) =>
        await HandleComposerKeyAsync(e);

    private async Task HandleComposerKeyAsync(KeyRoutedEventArgs e)
    {
        if (e.Key == VirtualKey.Escape)
        {
            if (_editingEventId is null && _replyToEventId is null) return;
            e.Handled = true;
            CancelBanner_Click(this, e);
            return;
        }
        if (e.Key != VirtualKey.Enter) return;
        if (IsDown(VirtualKey.Shift)) return;
        e.Handled = true;
        await SubmitComposerAsync();
    }

    private static bool IsDown(VirtualKey key) =>
        InputKeyboardSource.GetKeyStateForCurrentThread(key).HasFlag(CoreVirtualKeyStates.Down);

    private async void Send_Click(object sender, RoutedEventArgs e) => await SubmitComposerAsync();

    private async Task SubmitComposerAsync()
    {
        string text = ComposerInput.Text.Trim();
        if (_submitting || text.Length == 0 || _snapshot?.SelectedContactId is not string userId) return;
        _submitting = true;
        try
        {
            bool ok = _editingEventId is null
                ? await ExecuteAsync(new
                {
                    command = "send_text", user_id = userId, text, reply_to_event_id = _replyToEventId,
                })
                : await ExecuteAsync(new { command = "edit_message", event_id = _editingEventId, text });
            if (!ok) return;
            ComposerInput.Text = string.Empty;
            _editingEventId = null;
            _replyToEventId = null;
            UpdateBanner();
            if (_feed.Count > 0) MessagesList.ScrollIntoView(_feed[^1]);
        }
        finally
        {
            _submitting = false;
        }
    }

    private void BeginEdit(MessageModel message)
    {
        _editingEventId = message.EventId;
        _replyToEventId = null;
        ComposerInput.Text = message.Text;
        MessagesList.SelectedItems.Clear();
        UpdateBanner();
        ComposerInput.Focus(FocusState.Programmatic);
        ComposerInput.SelectionStart = ComposerInput.Text.Length;
    }

    /// <summary>Панель над полем ввода показывает правку или цитату ответа.</summary>
    private void UpdateBanner()
    {
        MessageModel? message = null;
        if (_editingEventId is not null)
        {
            message = _snapshot?.Messages.FirstOrDefault(value => value.EventId == _editingEventId);
            BannerTitle.Text = "Редактирование";
            BannerIcon.Symbol = Symbol.Edit;
        }
        else if (_replyToEventId is not null)
        {
            message = _snapshot?.Messages.FirstOrDefault(value => value.EventId == _replyToEventId);
            BannerTitle.Text = message?.Outgoing == true ? "Вы" : _snapshot?.SelectedChat?.DisplayName ?? "Ответ";
            BannerIcon.Symbol = Symbol.MailReply;
        }
        BannerText.Text = message?.Quote ?? string.Empty;
        ComposerBanner.Visibility = message is null ? Visibility.Collapsed : Visibility.Visible;
    }

    private void CancelBanner_Click(object sender, RoutedEventArgs e)
    {
        if (_editingEventId is not null) ComposerInput.Text = string.Empty;
        _editingEventId = null;
        _replyToEventId = null;
        UpdateBanner();
    }

    /// <summary>Черновик Telegram переживает переключение чатов.</summary>
    private void PersistDraft()
    {
        if (_draftChatId is not string userId || _snapshot is null) return;
        ChatModel? chat = _snapshot.Chats.FirstOrDefault(value => value.UserId == userId);
        if (chat is null || _editingEventId is not null) return;
        string draft = ComposerInput.Text.Trim();
        if (draft == chat.Draft) return;
        _ = _core.Invoke(new { command = "save_draft", user_id = userId, text = draft });
    }

    // --- панель выделения -----------------------------------------------------

    private void ClearSelection_Click(object sender, RoutedEventArgs e) => ExitMessageSelectionMode();

    private void EnterMessageSelectionMode(MessageModel message)
    {
        _messageSelectionMode = true;
        MessagesList.SelectionMode = ListViewSelectionMode.Multiple;
        MessagesList.SelectedItems.Add(message);
    }

    private void ExitMessageSelectionMode()
    {
        _messageSelectionMode = false;
        MessagesList.SelectedItems.Clear();
        MessagesList.SelectionMode = ListViewSelectionMode.None;
    }

    private void Copy_Click(object sender, RoutedEventArgs e)
    {
        string text = string.Join(Environment.NewLine,
            SelectedMessages().Where(value => !value.Deleted).Select(value => value.Text));
        CopyText(text);
        MessagesList.SelectedItems.Clear();
    }

    private static void CopyText(string text)
    {
        if (string.IsNullOrEmpty(text)) return;
        var package = new DataPackage();
        package.SetText(text);
        Clipboard.SetContent(package);
    }

    private void ReactSelected_Click(object sender, RoutedEventArgs e)
    {
        string[] ids = [.. SelectedMessages().Select(value => value.EventId)];
        if (ids.Length == 0) return;
        var menu = new MenuFlyout();
        foreach (string reaction in ReactionSet)
        {
            string value = reaction;
            var item = new MenuFlyoutItem { Text = value };
            item.Click += async (_, _) =>
            {
                MessagesList.SelectedItems.Clear();
                await ExecuteAsync(new { command = "react", event_ids = ids, reaction = value });
            };
            menu.Items.Add(item);
        }
        menu.ShowAt((FrameworkElement)sender);
    }

    private void EditSelected_Click(object sender, RoutedEventArgs e)
    {
        if (SelectedMessages() is [{ Outgoing: true, Deleted: false } message]) BeginEdit(message);
    }

    private async void DeleteSelected_Click(object sender, RoutedEventArgs e)
    {
        string[] ids = [.. SelectedMessages().Where(value => value.Outgoing).Select(value => value.EventId)];
        MessagesList.SelectedItems.Clear();
        if (ids.Length > 0) await ExecuteAsync(new { command = "delete_messages", event_ids = ids });
    }

    private void ForwardSelected_Click(object sender, RoutedEventArgs e)
    {
        string[] ids = [.. SelectedMessages().Select(value => value.EventId)];
        MessagesList.SelectedItems.Clear();
        ShowForwardDialog(ids);
    }

    private async void ShowForwardDialog(string[] eventIds)
    {
        if (eventIds.Length == 0 || _snapshot is null) return;
        _forwardEventIds = eventIds;
        _forwardTargets.Clear();
        foreach (ChatModel chat in _snapshot.Chats.Where(value => !value.PendingApproval)) _forwardTargets.Add(chat);
        ForwardList.SelectedItem = null;
        await ForwardDialog.ShowAsync();
    }

    private async void ForwardDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        if (ForwardList.SelectedItem is not ChatModel target)
        {
            args.Cancel = true;
            return;
        }
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        await ExecuteAsync(new { command = "forward_messages", event_ids = _forwardEventIds, user_id = target.UserId });
        deferral.Complete();
    }

    // --- шапка диалога --------------------------------------------------------

    private void ChatMenu_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedChat is not ChatModel chat) return;
        var menu = new MenuFlyout();
        menu.Items.Add(MenuItem("Профиль", "", ShowProfile));
        menu.Items.Add(MenuItem(chat.MuteMenuLabel, "", async () =>
            await ExecuteAsync(new { command = "set_chat_muted", user_id = chat.UserId, muted = !chat.Muted })));
        menu.Items.Add(MenuItem(chat.PinMenuLabel, "", async () =>
            await ExecuteAsync(new { command = "set_chat_pinned", user_id = chat.UserId, pinned = !chat.Pinned })));
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(MenuItem("Очистить историю", "", async () =>
        {
            if (await ConfirmAsync("Очистить историю?", "Локальные сообщения этого диалога будут удалены."))
                await ExecuteAsync(new { command = "clear_history", user_id = chat.UserId });
        }));
        menu.Items.Add(MenuItem("Удалить чат", "", async () =>
        {
            if (await ConfirmAsync("Удалить диалог?", "Локальная история этого диалога будет удалена."))
                await ExecuteAsync(new { command = "delete_contact", user_id = chat.UserId });
        }));
        menu.ShowAt((FrameworkElement)sender);
    }

    private void ContactProfile_Tapped(object sender, TappedRoutedEventArgs e) => ShowProfile();

    private async void AcceptContact_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedContactId is string id) await ExecuteAsync(new { command = "accept_contact", user_id = id });
    }

    private async void RejectContact_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedContactId is string id) await ExecuteAsync(new { command = "reject_contact", user_id = id });
    }

    // --- профиль собеседника --------------------------------------------------

    private void ShowProfile()
    {
        if (_snapshot?.SelectedChat is null) return;
        FillProfile();
        ProfilePage.Visibility = Visibility.Visible;
    }

    private void FillProfile()
    {
        if (_snapshot?.SelectedChat is not ChatModel chat) return;
        _updating = true;
        ProfileInitial.Text = chat.Initials;
        ProfileAvatar.Source = Images.Decode(chat.AvatarBase64);
        ProfileName.Text = chat.DisplayName;
        ProfilePresence.Text = chat.Presence;
        ProfileUsername.Text = chat.Username is null ? "Username не задан" : "@" + chat.Username;
        ProfileUserId.Text = chat.UserId;
        ProfileAbout.Text = chat.About ?? string.Empty;
        VerifiedToggle.IsOn = chat.FingerprintVerified;
        _updating = false;
    }

    private void CloseProfile_Click(object sender, RoutedEventArgs e) => ProfilePage.Visibility = Visibility.Collapsed;

    private async void Verified_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updating || _snapshot?.SelectedContactId is not string id) return;
        await ExecuteAsync(new { command = "verify_contact", user_id = id, verified = VerifiedToggle.IsOn });
    }

    private async void ClearHistory_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedContactId is not string id) return;
        if (await ConfirmAsync("Очистить историю?", "Локальные сообщения этого диалога будут удалены."))
            await ExecuteAsync(new { command = "clear_history", user_id = id });
    }

    private async void DeleteChat_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedContactId is not string id) return;
        if (!await ConfirmAsync("Удалить диалог?", "Локальная история этого диалога будет удалена.")) return;
        ProfilePage.Visibility = Visibility.Collapsed;
        await ExecuteAsync(new { command = "delete_contact", user_id = id });
    }

    // --- настройки ------------------------------------------------------------

    private void Settings_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot is null) return;
        FillSettings();
        ShowSettingsCategory("profile");
        SettingsPage.Visibility = Visibility.Visible;
    }

    private void FillSettings()
    {
        if (_snapshot is null) return;
        _updating = true;
        SettingsInitials.Text = Formatting.Initials(
            _snapshot.Profile.DisplayName.Length > 0 ? _snapshot.Profile.DisplayName : "Turat");
        SettingsAvatar.Source = Images.Decode(_snapshot.Profile.AvatarBase64);
        SettingsName.Text = _snapshot.Profile.DisplayName.Length > 0 ? _snapshot.Profile.DisplayName : "Без имени";
        SettingsState.Text = _snapshot.Online ? "в сети · сквозное шифрование" : "локальный режим";
        MyUserId.Text = _snapshot.Identity.UserId;
        SettingsUsername.Text = _snapshot.Profile.Username;
        SettingsDisplayName.Text = _snapshot.Profile.DisplayName;
        SettingsAbout.Text = _snapshot.Profile.About;
        BootstrapInput.Text = _snapshot.Settings.BootstrapUrl;
        MetadataButton.Content = "Метаданные: " + _snapshot.Settings.MetadataProtection;
        PresenceToggle.IsOn = _snapshot.Settings.PublishPresence;
        NetworkHint.Text = _snapshot.Online
            ? "Клиент подключается к Node сам и обновляет диалоги в фоне."
            : "Нет связи с Node. Проверьте адрес и подключение — клиент повторит попытку сам.";
        NetworkHint.Foreground = (Brush)Application.Current.Resources[_snapshot.Online ? "TgHint" : "TgDanger"];
        ThemeName.Text = _theme.Title;
        ThemeList.SelectedItem = _themes.FirstOrDefault(theme => theme.Id == _theme.Id);
        FontName.Text = _font.Title;
        FontList.SelectedItem = _fonts.FirstOrDefault(font => font.Id == _font.Id);
        _updating = false;
    }

    private void SettingsCategory_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: string category }) ShowSettingsCategory(category);
    }

    /// <summary>В настройках видна только одна компактная категория; выбор оформлен как стеклянные сегменты.</summary>
    private void ShowSettingsCategory(string category)
    {
        ProfileSettingsPanel.Visibility = category == "profile" ? Visibility.Visible : Visibility.Collapsed;
        AppearanceSettingsPanel.Visibility = category == "appearance" ? Visibility.Visible : Visibility.Collapsed;
        ConnectionSettingsPanel.Visibility = category == "connection" ? Visibility.Visible : Visibility.Collapsed;
        DataSettingsPanel.Visibility = category == "data" ? Visibility.Visible : Visibility.Collapsed;

        foreach ((Button button, string id) in new[]
        {
            (ProfileCategoryButton, "profile"),
            (AppearanceCategoryButton, "appearance"),
            (ConnectionCategoryButton, "connection"),
            (DataCategoryButton, "data"),
        })
        {
            bool selected = id == category;
            button.Background = (Brush)Application.Current.Resources[selected ? "TgAccentSoft" : "TgElevated"];
            button.Foreground = (Brush)Application.Current.Resources[selected ? "TgText" : "TgHint"];
            button.Opacity = selected ? 1 : 0.82;
        }
        SettingsScroll.ChangeView(null, 0, null, disableAnimation: false);
    }

    private void CloseSettings_Click(object sender, RoutedEventArgs e) => SettingsPage.Visibility = Visibility.Collapsed;

    private async void SaveProfile_Click(object sender, RoutedEventArgs e) =>
        await SaveProfileAsync(_snapshot?.Profile.AvatarBase64);

    /// <summary>Профиль сохраняется локально и затем публикуется в directory Node.</summary>
    private async Task SaveProfileAsync(string? avatar)
    {
        bool saved = await ExecuteAsync(new
        {
            command = "save_profile",
            username = SettingsUsername.Text,
            display_name = SettingsDisplayName.Text,
            about = SettingsAbout.Text,
            avatar_base64 = avatar,
        });
        if (saved) await ExecuteAsync(new { command = "publish_profile" });
    }

    private async void Presence_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updating) return;
        await ExecuteAsync(new { command = "set_presence_publishing", enabled = PresenceToggle.IsOn });
    }

    private async void PickAvatar_Click(object sender, RoutedEventArgs e)
    {
        StorageFile? file = await OpenFileAsync([".png", ".jpg", ".jpeg", ".bmp"]);
        if (file is null) return;
        string? encoded = await EncodeAvatarAsync(file);
        if (encoded is null)
        {
            await ShowErrorAsync("Не удалось прочитать изображение");
            return;
        }
        await SaveProfileAsync(encoded);
    }

    /// <summary>Аватар уменьшается до 256 px и кодируется в JPEG: запись профиля должна быть компактной.</summary>
    private static async Task<string?> EncodeAvatarAsync(StorageFile file)
    {
        try
        {
            using IRandomAccessStream input = await file.OpenAsync(FileAccessMode.Read);
            BitmapDecoder decoder = await BitmapDecoder.CreateAsync(input);
            double scale = Math.Min(1d, 256d / Math.Max(decoder.PixelWidth, decoder.PixelHeight));
            uint width = (uint)Math.Max(1, decoder.PixelWidth * scale);
            uint height = (uint)Math.Max(1, decoder.PixelHeight * scale);
            PixelDataProvider pixels = await decoder.GetPixelDataAsync(
                BitmapPixelFormat.Bgra8,
                BitmapAlphaMode.Ignore,
                new BitmapTransform { ScaledWidth = width, ScaledHeight = height, InterpolationMode = BitmapInterpolationMode.Fant },
                ExifOrientationMode.RespectExifOrientation,
                ColorManagementMode.DoNotColorManage);

            using var output = new InMemoryRandomAccessStream();
            BitmapEncoder encoder = await BitmapEncoder.CreateAsync(BitmapEncoder.JpegEncoderId, output);
            encoder.SetPixelData(BitmapPixelFormat.Bgra8, BitmapAlphaMode.Ignore, width, height, 96, 96,
                pixels.DetachPixelData());
            await encoder.FlushAsync();

            var reader = new DataReader(output.GetInputStreamAt(0));
            await reader.LoadAsync((uint)output.Size);
            byte[] bytes = new byte[output.Size];
            reader.ReadBytes(bytes);
            return Convert.ToBase64String(bytes);
        }
        catch
        {
            return null;
        }
    }

    private async void Connect_Click(object sender, RoutedEventArgs e)
    {
        if (await ExecuteAsync(new { command = "connect", bootstrap_url = BootstrapInput.Text.Trim() }))
        {
            ScheduleNextSync();
        }
    }

    private async void Metadata_Click(object sender, RoutedEventArgs e) =>
        await ExecuteAsync(new { command = "cycle_metadata_protection" });

    private async void RevokeDevice_Click(object sender, RoutedEventArgs e)
    {
        string deviceId = RevokeDeviceInput.Text.Trim();
        if (deviceId.Length == 0)
        {
            await ShowErrorAsync("Введите DeviceID устройства, которое нужно отозвать");
            return;
        }
        if (!await ConfirmAsync("Отозвать устройство?", "Устройство потеряет доступ к вашим диалогам.")) return;
        if (await ExecuteAsync(new { command = "revoke_device", device_id = deviceId }))
        {
            RevokeDeviceInput.Text = string.Empty;
        }
    }

    private async void Sync_Click(object sender, RoutedEventArgs e)
    {
        await ExecuteAsync(new { command = "sync" });
        ScheduleNextSync();
    }

    // --- оформление -----------------------------------------------------------

    /// <summary>Кнопка в рельсе открывает настройки на разделе оформления.</summary>
    private void Theme_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot is null) return;
        FillSettings();
        ShowSettingsCategory("appearance");
        SettingsPage.Visibility = Visibility.Visible;
    }

    private void ThemeList_ItemClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is ThemePalette theme && theme.Id != _theme.Id)
        {
            ApplyTheme(theme);
            UiSettings.ThemeId = theme.Id;
        }
    }

    private void ApplyTheme(ThemePalette theme)
    {
        _theme = theme;
        ThemeCatalog.Apply(theme);
        // Встроенные элементы WinUI (поля, диалоги, полосы прокрутки) берут цвета у системы:
        // светлой теме нужна светлая системная схема, иначе текст в них становится нечитаемым.
        Root.RequestedTheme = theme.Dark ? ElementTheme.Dark : ElementTheme.Light;
        ToolTipService.SetToolTip(ThemeButton, "Тема: " + theme.Title);
        ThemeName.Text = theme.Title;
        ThemeList.SelectedItem = theme;
        ApplyTitleBarColors(theme);
    }

    private void FontList_ItemClick(object sender, ItemClickEventArgs e)
    {
        if (e.ClickedItem is AppFontChoice font && font.Id != _font.Id)
        {
            ApplyFont(font);
            UiSettings.FontId = font.Id;
        }
    }

    /// <summary>Меняет гарнитуру уже созданных элементов; FontFamily списков наследуют и новые строки.</summary>
    private void ApplyFont(AppFontChoice font)
    {
        _font = font;
        FontName.Text = font.Title;
        FontList.SelectedItem = font;
        ApplyFontToTree(Root, font.Family);
    }

    private void ApplyFontToTree(DependencyObject element, FontFamily family)
    {
        // У каждой карточки списка свой FontFamily: это живое превью, его нельзя заменить
        // выбранной глобальной гарнитурой вместе с остальным интерфейсом.
        if (ReferenceEquals(element, FontList)) return;

        // Пиктограммы сами управляют служебной гарнитурой. Не заходим и в их
        // внутреннее дерево, иначе выбранный текстовый шрифт превращает их в квадраты.
        if (element is IconElement) return;

        switch (element)
        {
            case TextBlock text when text.FontFamily.Source != "Consolas":
                text.FontFamily = family;
                break;
            case TextBox textBox:
                textBox.FontFamily = family;
                break;
            case PasswordBox passwordBox:
                passwordBox.FontFamily = family;
                break;
            case ToggleSwitch toggleSwitch:
                toggleSwitch.FontFamily = family;
                break;
            case Button { Content: string } button:
                button.FontFamily = family;
                break;
            case MenuFlyoutItem menuItem:
                menuItem.FontFamily = family;
                break;
        }

        for (int index = 0; index < VisualTreeHelper.GetChildrenCount(element); index++)
            ApplyFontToTree(VisualTreeHelper.GetChild(element, index), family);
    }

    /// <summary>Кнопки закрытия и сворачивания рисует система: их цвета задаются отдельно.</summary>
    private void ApplyTitleBarColors(ThemePalette theme)
    {
        try
        {
            Microsoft.UI.Windowing.AppWindowTitleBar bar = AppWindow.TitleBar;
            bar.ButtonBackgroundColor = Colors.Transparent;
            bar.ButtonInactiveBackgroundColor = Colors.Transparent;
            bar.ButtonForegroundColor = ThemeCatalog.ToColor(theme.Text);
            bar.ButtonInactiveForegroundColor = ThemeCatalog.ToColor(theme.Hint);
            bar.ButtonHoverBackgroundColor = ThemeCatalog.ToColor(theme.RowHover);
            bar.ButtonHoverForegroundColor = ThemeCatalog.ToColor(theme.Text);
            bar.ButtonPressedBackgroundColor = ThemeCatalog.ToColor(theme.Field);
            bar.ButtonPressedForegroundColor = ThemeCatalog.ToColor(theme.Text);
        }
        catch
        {
            // Оформление системных кнопок доступно не во всех сборках Windows.
        }
    }

    // --- новый диалог и первый запуск ----------------------------------------

    private async void NewContact_Click(object sender, RoutedEventArgs e)
    {
        NewContactInput.Text = string.Empty;
        NewContactError.Text = string.Empty;
        NewContactError.Visibility = Visibility.Collapsed;
        await NewContactDialog.ShowAsync();
    }

    private async void NewContactDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        NewContactError.Text = string.Empty;
        NewContactError.Visibility = Visibility.Collapsed;
        bool added = await ExecuteAsync(new
        {
            command = "add_contact", query = NewContactInput.Text, display_name = (string?)null,
        }, showErrorDialog: false);
        args.Cancel = !added;
        if (!added)
        {
            NewContactError.Text = _snapshot?.StatusMessage ?? "Не удалось найти пользователя";
            NewContactError.Visibility = Visibility.Visible;
        }
        deferral.Complete();
    }

    private async Task ShowOnboardingAsync()
    {
        await Task.Yield();
        if (_snapshot?.OnboardingRequired == true) await OnboardingDialog.ShowAsync();
    }

    private async void OnboardingDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        args.Cancel = !await ExecuteAsync(new
        {
            command = "save_profile",
            username = OnboardingUsername.Text,
            display_name = OnboardingDisplayName.Text,
            about = string.Empty,
            avatar_base64 = (string?)null,
        });
        deferral.Complete();
    }

    // --- вложения и пакеты ----------------------------------------------------

    private async void CreateBackup_Click(object sender, RoutedEventArgs e) => await ExportSecretAsync("create_backup", "Turat.ttbackup", ".ttbackup");
    private async void RestoreBackup_Click(object sender, RoutedEventArgs e) => await ImportSecretAsync("restore_backup", [".ttbackup"]);
    private async void CreateDeviceLink_Click(object sender, RoutedEventArgs e) => await ExportSecretAsync("create_device_link", "Turat.ttlink", ".ttlink");
    private async void ImportDeviceLink_Click(object sender, RoutedEventArgs e) => await ImportSecretAsync("import_device_link", [".ttlink"]);
    private async void ExportPortable_Click(object sender, RoutedEventArgs e) => await ExportFileAsync("export_portable", "messages.ttenv", ".ttenv");
    private async void ImportPortable_Click(object sender, RoutedEventArgs e) => await ImportFileAsync("import_portable", [".ttenv"]);
    private async void ExportDiscovery_Click(object sender, RoutedEventArgs e) => await ExportFileAsync("export_discovery", "network.ttbridge", ".ttbridge");
    private async void ImportDiscovery_Click(object sender, RoutedEventArgs e) => await ImportFileAsync("import_discovery", [".ttbridge"]);

    private async Task ExportSecretAsync(string command, string name, string extension)
    {
        StorageFile? file = await SaveFileAsync(name, extension);
        if (file is not null) await ExecuteAsync(new { command, path = file.Path, passphrase = SecurityPassphrase.Password });
    }

    private async Task ImportSecretAsync(string command, IReadOnlyList<string> extensions)
    {
        StorageFile? file = await OpenFileAsync(extensions);
        if (file is not null) await ExecuteAsync(new { command, path = file.Path, passphrase = SecurityPassphrase.Password });
    }

    private async Task ExportFileAsync(string command, string name, string extension)
    {
        StorageFile? file = await SaveFileAsync(name, extension);
        if (file is not null) await ExecuteAsync(new { command, path = file.Path });
    }

    private async Task ImportFileAsync(string command, IReadOnlyList<string> extensions)
    {
        StorageFile? file = await OpenFileAsync(extensions);
        if (file is not null) await ExecuteAsync(new { command, path = file.Path });
    }

    private async Task<StorageFile?> OpenFileAsync(IReadOnlyList<string> extensions)
    {
        var picker = new FileOpenPicker();
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        foreach (string extension in extensions) picker.FileTypeFilter.Add(extension);
        return await picker.PickSingleFileAsync();
    }

    private async Task<StorageFile?> SaveFileAsync(string name, string extension)
    {
        var picker = new FileSavePicker { SuggestedFileName = Path.GetFileNameWithoutExtension(name) };
        picker.FileTypeChoices.Add("Turat", [extension]);
        InitializeWithWindow.Initialize(picker, WindowNative.GetWindowHandle(this));
        return await picker.PickSaveFileAsync();
    }

    // --- мелкие помощники -----------------------------------------------------

    private static MenuFlyoutItem MenuItem(string text, string glyph, Action action)
    {
        var item = new MenuFlyoutItem
        {
            Text = text,
            Icon = new FontIcon { Glyph = glyph, FontFamily = IconFontFamily },
        };
        item.Click += (_, _) => action();
        return item;
    }

    private static MenuFlyoutItem MessageMenuItem(string text, string glyph, Action action)
    {
        MenuFlyoutItem item = MenuItem(text, glyph, action);
        item.Style = (Style)Application.Current.Resources["TgMessageMenuItem"];
        return item;
    }

    private static void ShowMenu(MenuFlyout menu, UIElement sender, ContextRequestedEventArgs args)
    {
        if (sender is not FrameworkElement element) return;
        if (args.TryGetPosition(element, out global::Windows.Foundation.Point position))
        {
            menu.ShowAt(element, new FlyoutShowOptions { Position = position });
        }
        else
        {
            menu.ShowAt(element);
        }
        args.Handled = true;
    }

    private async Task<bool> ConfirmAsync(string title, string content)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = Root.XamlRoot,
            RequestedTheme = Root.RequestedTheme,
            Title = title,
            Content = content,
            PrimaryButtonText = "Да",
            CloseButtonText = "Отмена",
            DefaultButton = ContentDialogButton.Close,
        };
        return await dialog.ShowAsync() == ContentDialogResult.Primary;
    }

    private async Task ShowErrorAsync(string message)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = Root.XamlRoot,
            RequestedTheme = Root.RequestedTheme,
            Title = "Turat",
            Content = message,
            CloseButtonText = "Закрыть",
        };
        await dialog.ShowAsync();
    }

    /// <summary>
    /// Ошибка может прийти из обработчика уже открытого ContentDialog. WinUI запрещает второй
    /// диалог на том же XamlRoot; исключение из async void раньше завершало весь процесс.
    /// </summary>
    private async Task TryShowErrorAsync(string message)
    {
        try
        {
            await ShowErrorAsync(message);
        }
        catch (Exception)
        {
            StatusText.Text = message;
        }
    }
}
