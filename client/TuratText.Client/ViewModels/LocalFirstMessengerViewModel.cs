using System.Collections.ObjectModel;
using Avalonia.Media.Imaging;
using Avalonia.Threading;
using TuratText.Client.LocalFirst;
using TuratText.Client.Messaging.V2;
using TuratText.Client.Transport.V2;
using TuratText.Client.Ui;
using TuratText.Client.Updates;

namespace TuratText.Client.ViewModels;

public sealed class LocalFirstMessengerViewModel : ViewModelBase
{
    private readonly MainWindowViewModel _main;
    private readonly ProtocolIdentityService _identity;
    private readonly LocalFirstRuntime _runtime;
    private readonly MailboxProvisioningService _provisioning;
    private readonly BootstrapNodeSource _bootstrapNodes;
    private readonly OwnedMailboxStore _mailboxes;
    private readonly RoutingNodeClient _routing;
    private readonly UsernameDirectoryClient _usernames;
    private readonly ProfileDirectoryClient _profiles;
    private readonly LocalContactStore _contactStore;
    private readonly V2MessagingService _messaging;
    private readonly LocalFirstBackupService _backup;
    private readonly DeviceLinkService _deviceLink;
    private readonly PortableEnvelopeService _portableEnvelopes;
    private readonly DiscoveryBundleService _discovery;
    private readonly ThresholdUpdateClient _updates;
    private readonly MetadataProtectionSettings _metadataProtection;
    private readonly CancellationTokenSource _pollCancellation = new();
    private readonly SemaphoreSlim _syncGate = new(1, 1);
    private readonly ObservableCollection<LocalTextMessage> _selectedMessages = new();
    private readonly Dictionary<string, Bitmap> _decryptedImageCache = new();
    private readonly HashSet<string> _thumbnailsLoading = new();
    private List<LocalTextMessage> _lightboxItems = new();
    private List<LocalTextMessage> _currentHistory = new();
    private string? _currentHistoryContactId;
    private readonly Dictionary<string, DateTimeOffset> _lastSeenByUserId = new(StringComparer.Ordinal);
    private DateTimeOffset _lastPresenceSentAt = DateTimeOffset.MinValue;
    private int _lightboxIndex;
    private DateTimeOffset _lastInteraction = DateTimeOffset.UtcNow;
    private DateTimeOffset _lastMaintenance = DateTimeOffset.UtcNow;
    private LocalContact? _selectedContact;
    private LocalTextMessage? _selectedMessage;
    private LocalTextMessage? _editingMessage;
    private string _bootstrapUrl;
    private string _newContactUserId = "";
    private string _myUsername = "";
    private string _newMessage = "";
    private string _securityPassphrase = "";
    private string _revokeDeviceId = "";
    private string _statusMessage = "Подключение к TuratText…";
    private string _contactQuery = "";
    private string _profileDisplayName = "";
    private string _profileAbout = "";
    private string? _profileAvatarMimeType;
    private string? _profileAvatarBase64;
    private Bitmap? _profileAvatarPreview;
    private string _contactProfileUsername = "";
    private string _contactProfileAbout = "";
    private Bitmap? _contactProfileAvatar;
    private Bitmap? _lightboxImage;
    private bool _isBusy;
    private bool _isChatOpen;
    private bool _isNewContactOpen;
    private bool _isSettingsOpen;
    private bool _isOnboardingOpen;
    private bool _isContactProfileOpen;
    private bool _isLightboxOpen;
    private bool _isOnline;
    private bool _measuredOnce;
    private double _viewportWidth = 1180;
    private MetadataProtectionMode _metadataMode;

    /// <summary>Below this width the shell collapses into a single-pane, phone-friendly layout.</summary>
    public const double CompactWidthThreshold = 880;

    private static readonly TimeSpan ActiveSyncInterval = TimeSpan.FromMilliseconds(900);
    private static readonly TimeSpan IdleAfter = TimeSpan.FromMinutes(3);
    private static readonly TimeSpan PresenceHeartbeatInterval = TimeSpan.FromMinutes(2);
    // Two missed heartbeats before we call it stale, so one delayed/lost delivery doesn't flip the
    // header to "last seen" and back a few seconds later.
    private static readonly TimeSpan PresenceFreshWindow = TimeSpan.FromMinutes(5);

    public LocalFirstMessengerViewModel(
        MainWindowViewModel main,
        ProtocolIdentityService identity,
        LocalFirstRuntime runtime,
        MailboxProvisioningService provisioning,
        BootstrapNodeSource bootstrapNodes,
        OwnedMailboxStore mailboxes,
        RoutingNodeClient routing,
        UsernameDirectoryClient usernames,
        ProfileDirectoryClient profiles,
        LocalContactStore contactStore,
        V2MessagingService messaging,
        LocalFirstBackupService backup,
        DeviceLinkService deviceLink,
        PortableEnvelopeService portableEnvelopes,
        DiscoveryBundleService discovery,
        ThresholdUpdateClient updates,
        MetadataProtectionSettings metadataProtection,
        string? bootstrapUrl)
    {
        _main = main;
        _identity = identity;
        _runtime = runtime;
        _provisioning = provisioning;
        _bootstrapNodes = bootstrapNodes;
        _mailboxes = mailboxes;
        _routing = routing;
        _usernames = usernames;
        _profiles = profiles;
        _contactStore = contactStore;
        _messaging = messaging;
        _backup = backup;
        _deviceLink = deviceLink;
        _portableEnvelopes = portableEnvelopes;
        _discovery = discovery;
        _updates = updates;
        _metadataProtection = metadataProtection;
        _messaging.PresenceReceived += OnPresenceReceived;
        SelectedMessages = new ReadOnlyObservableCollection<LocalTextMessage>(_selectedMessages);
        _bootstrapUrl = Uri.TryCreate(bootstrapUrl, UriKind.Absolute, out _)
            ? bootstrapUrl!
            : BootstrapNodeSource.DefaultBootstrapUrl;
        ConnectCommand = new AsyncRelayCommand(ConnectAsync);
        AddContactCommand = new AsyncRelayCommand(AddContactAsync);
        PublishUsernameCommand = new AsyncRelayCommand(PublishUsernameAsync);
        CompleteOnboardingCommand = new AsyncRelayCommand(CompleteOnboardingAsync);
        SaveProfileCommand = new AsyncRelayCommand(SaveProfileAsync);
        OpenContactProfileCommand = new AsyncRelayCommand(OpenContactProfileAsync);
        LightboxNextCommand = new AsyncRelayCommand(LightboxNextAsync);
        LightboxPrevCommand = new AsyncRelayCommand(LightboxPrevAsync);
        CloseLightboxCommand = new RelayCommand(CloseLightbox);
        SendCommand = new AsyncRelayCommand(SendAsync);
        EditCommand = new AsyncRelayCommand(EditAsync);
        SubmitComposerCommand = new AsyncRelayCommand(SubmitComposerAsync);
        DeleteCommand = new AsyncRelayCommand(DeleteAsync);
        ReactCommand = new AsyncRelayCommand(ReactAsync);
        AcceptContactCommand = new AsyncRelayCommand(AcceptContactAsync);
        RejectContactCommand = new AsyncRelayCommand(RejectContactAsync);
        DeleteChatCommand = new AsyncRelayCommand<LocalContact>(DeleteChatAsync);
        RefreshCommand = new AsyncRelayCommand(RefreshAsync);
        CheckUpdateCommand = new AsyncRelayCommand(CheckUpdateAsync);
        CycleMetadataModeCommand = new AsyncRelayCommand(CycleMetadataModeAsync);
        RevokeDeviceCommand = new AsyncRelayCommand(RevokeDeviceAsync);
        BackToChatsCommand = new RelayCommand(BackToChats);
        OpenNewContactCommand = new RelayCommand(() => IsNewContactOpen = true);
        OpenSettingsCommand = new RelayCommand(() => IsSettingsOpen = true);
        CloseOverlayCommand = new RelayCommand(CloseOverlays);
        BeginEditCommand = new RelayCommand(BeginEdit);
        CancelEditCommand = new RelayCommand(CancelEdit);
        ClearMessageSelectionCommand = new RelayCommand(ClearMessageSelection);
    }

    public ObservableCollection<LocalContact> Contacts { get; } = new();
    public ObservableCollection<LocalContact> VisibleContacts { get; } = new();
    public ObservableCollection<MessageRow> MessageRows { get; } = new();
    public ReadOnlyObservableCollection<LocalTextMessage> SelectedMessages { get; }

    public string UserId => _identity.Current?.UserId ?? "identity loading";
    public string DeviceId => _identity.Current?.DeviceId ?? "device loading";
    public string UserIdShort => ShortId(UserId);
    public bool IsMobile => _main.IsMobile;
    public bool IsDesktop => !_main.IsMobile;

    /// <summary>Width of the surface hosting the messenger, pushed in by the view on every resize.</summary>
    public double ViewportWidth
    {
        get => _viewportWidth;
        set
        {
            if (value <= 0 || Math.Abs(value - _viewportWidth) < 0.5) return;
            bool wasWide = IsWide;
            _viewportWidth = value;
            OnPropertyChanged(nameof(ViewportWidth));

            if (!_measuredOnce)
            {
                // First real measurement: a narrow window should open on the chat list, not inside a
                // conversation that was auto-selected while the width was still unknown.
                _measuredOnce = true;
                if (!IsWide)
                {
                    _isChatOpen = false;
                    SelectContact(null);
                }
            }
            else if (wasWide != IsWide)
            {
                if (IsWide && SelectedContact is null) SelectContact(VisibleContacts.FirstOrDefault());
                if (!IsWide && SelectedContact is not null) _isChatOpen = true;
            }

            if (wasWide != IsWide)
            {
                OnPropertyChanged(nameof(IsWide));
                OnPropertyChanged(nameof(IsCompact));
                OnPropertyChanged(nameof(IsChatOpen));
            }
            NotifyShellLayoutChanged();
        }
    }

    public bool IsWide => _viewportWidth >= CompactWidthThreshold;
    public bool IsCompact => !IsWide;

    /// <summary>In compact mode the sidebar takes the whole surface; wide mode gets a fixed rail.</summary>
    public double SidebarWidth => IsWide ? 344 : Math.Max(_viewportWidth, 1);

    public bool ShowSidebar => IsWide || !IsChatOpen;
    public bool ShowConversation => IsWide || IsChatOpen;
    public bool ShowBackButton => IsCompact;

    public bool HasContacts => Contacts.Count > 0;
    public bool HasNoContacts => Contacts.Count == 0;
    public bool HasVisibleContacts => VisibleContacts.Count > 0;
    public bool HasNoSearchResults => Contacts.Count > 0 && VisibleContacts.Count == 0;
    public bool HasSelectedContact => SelectedContact is not null;
    public bool HasNoSelectedContact => SelectedContact is null;
    public bool HasMessages => SelectedContact is not null && MessageRows.Count > 0;
    public bool HasNoMessages => SelectedContact is not null && MessageRows.Count == 0;
    public bool HasSelectedMessage => SelectedMessage is not null;
    public bool HasSelectedMessages => _selectedMessages.Count > 0;
    public bool HasSingleSelectedMessage => _selectedMessages.Count == 1;
    public string SelectedMessageCountLabel => _selectedMessages.Count switch
    {
        1 => "Выбрано 1 сообщение",
        var count => $"Выбрано: {count}"
    };
    public bool HasSelectedAttachment => HasSingleSelectedMessage && SelectedMessage?.Attachment is not null;

    // Edit needs one unambiguous target; delete, react and copy work across the whole selection.
    public bool CanModifySelectedMessage => HasSingleSelectedMessage && SelectedMessage is { Outgoing: true, Deleted: false };
    public bool CanDeleteSelectedMessages => DeletableSelection.Count > 0;
    public bool CanReactSelectedMessage => ReactableSelection.Count > 0;
    public bool CanCopySelectedMessages => _selectedMessages.Any(message => !message.Deleted
                                                                            && !string.IsNullOrWhiteSpace(message.Text));
    public string DeleteSelectionLabel => Suffixed("Удалить", DeletableSelection.Count);
    public string ReactSelectionLabel => Suffixed("👍 Реакция", ReactableSelection.Count);
    public string CopySelectionLabel => Suffixed("Копировать", _selectedMessages.Count);
    public bool HasSelectedActions => HasSelectedMessages;

    private IReadOnlyList<LocalTextMessage> DeletableSelection =>
        _selectedMessages.Where(message => message is { Outgoing: true, Deleted: false }).ToList();

    private IReadOnlyList<LocalTextMessage> ReactableSelection =>
        _selectedMessages.Where(message => !message.Deleted).ToList();

    private static string Suffixed(string label, int count) => count > 1 ? $"{label} ({count})" : label;

    /// <summary>Text of the current selection, oldest first, ready for the clipboard.</summary>
    public string SelectedMessagesText => string.Join(
        Environment.NewLine,
        MessageRows
            .Where(row => IsMessageSelected(row.Message) && !row.Message.Deleted)
            .Select(row => row.Message.Text)
            .Where(text => !string.IsNullOrWhiteSpace(text)));
    public bool IsEditing => _editingMessage is not null;
    public string EditingPreview => _editingMessage?.Text ?? "";
    public bool CanSend => HasSelectedContact && !string.IsNullOrWhiteSpace(NewMessage);

    public bool IsChatOpen
    {
        get => _isChatOpen;
        private set
        {
            if (SetProperty(ref _isChatOpen, value)) NotifyShellLayoutChanged();
        }
    }

    public bool IsOnline
    {
        get => _isOnline;
        private set
        {
            if (SetProperty(ref _isOnline, value)) OnPropertyChanged(nameof(ConnectionLabel));
        }
    }

    public string ConnectionLabel => IsOnline ? "В сети · E2EE" : "Офлайн · локальный режим";

    /// <summary>Whether the open contact's most recent heartbeat is still within the fresh window.</summary>
    public bool SelectedContactPresenceOnline =>
        SelectedContact is not null
        && _lastSeenByUserId.TryGetValue(SelectedContact.UserId, out DateTimeOffset lastSeen)
        && DateTimeOffset.UtcNow - lastSeen < PresenceFreshWindow;

    /// <summary>False until this contact's first heartbeat arrives (e.g. never opened the chat yet,
    /// or is on an older client that doesn't send one) — hides the pill instead of showing nothing.</summary>
    public bool HasSelectedContactPresence =>
        SelectedContact is not null && _lastSeenByUserId.ContainsKey(SelectedContact.UserId);

    /// <summary>"В сети", "был(а) в сети …", or empty if this contact has never sent a heartbeat yet
    /// (e.g. they're on an older client, or simply haven't opened the chat since it was added).</summary>
    public string SelectedContactPresenceLabel
    {
        get
        {
            if (SelectedContact is null
                || !_lastSeenByUserId.TryGetValue(SelectedContact.UserId, out DateTimeOffset lastSeen))
            {
                return "";
            }
            return SelectedContactPresenceOnline ? "В сети" : "был(а) в сети " + FormatLastSeen(lastSeen);
        }
    }

    private static string FormatLastSeen(DateTimeOffset value)
    {
        DateTimeOffset local = value.ToLocalTime();
        TimeSpan age = DateTimeOffset.UtcNow - value;
        if (age < TimeSpan.FromMinutes(1)) return "только что";
        if (age < TimeSpan.FromHours(1)) return $"{(int)age.TotalMinutes} мин назад";
        DateTime today = DateTime.Today;
        if (local.Date == today) return "сегодня в " + local.ToString("HH:mm");
        if (local.Date == today.AddDays(-1)) return "вчера в " + local.ToString("HH:mm");
        return local.ToString("d MMMM в HH:mm", System.Globalization.CultureInfo.GetCultureInfo("ru-RU"));
    }

    /// <summary>Handles an incoming presence heartbeat. Fired from the background sync thread.</summary>
    private void OnPresenceReceived(string userId, DateTimeOffset at)
    {
        _ = Dispatcher.UIThread.InvokeAsync(() =>
        {
            if (_lastSeenByUserId.TryGetValue(userId, out DateTimeOffset existing) && existing >= at) return;
            _lastSeenByUserId[userId] = at;
            if (SelectedContact?.UserId == userId)
            {
                OnPropertyChanged(nameof(SelectedContactPresenceOnline));
                OnPropertyChanged(nameof(SelectedContactPresenceLabel));
                OnPropertyChanged(nameof(HasSelectedContactPresence));
            }
        });
    }

    /// <summary>Best-effort "I'm here" ping to the given contact; failures are silent since this is
    /// just a live status hint, not something the user is waiting on.</summary>
    private async Task SendPresenceHeartbeatAsync(LocalContact contact)
    {
        _lastPresenceSentAt = DateTimeOffset.UtcNow;
        try
        {
            await _messaging.SendPresenceAsync(contact);
        }
        catch
        {
            // The peer just won't see a fresh heartbeat this cycle; the next tick retries.
        }
    }

    public string ContactQuery
    {
        get => _contactQuery;
        set
        {
            if (SetProperty(ref _contactQuery, value)) ApplyContactFilter();
        }
    }

    public bool IsNewContactOpen
    {
        get => _isNewContactOpen;
        private set => SetProperty(ref _isNewContactOpen, value);
    }

    public bool IsSettingsOpen
    {
        get => _isSettingsOpen;
        private set => SetProperty(ref _isSettingsOpen, value);
    }

    public string BootstrapUrl
    {
        get => _bootstrapUrl;
        set => SetProperty(ref _bootstrapUrl, value);
    }

    public string NewContactUserId
    {
        get => _newContactUserId;
        set => SetProperty(ref _newContactUserId, value);
    }

    public string MyUsername
    {
        get => _myUsername;
        set => SetProperty(ref _myUsername, value);
    }

    public string ProfileDisplayName
    {
        get => _profileDisplayName;
        set => SetProperty(ref _profileDisplayName, value);
    }

    public string ProfileAbout
    {
        get => _profileAbout;
        set => SetProperty(ref _profileAbout, value);
    }

    public Bitmap? ProfileAvatarPreview
    {
        get => _profileAvatarPreview;
        private set
        {
            if (SetProperty(ref _profileAvatarPreview, value)) OnPropertyChanged(nameof(HasProfileAvatarPreview));
        }
    }

    public bool HasProfileAvatarPreview => ProfileAvatarPreview is not null;

    public string ContactProfileUsername
    {
        get => _contactProfileUsername;
        private set => SetProperty(ref _contactProfileUsername, value);
    }

    public bool HasContactProfileUsername => !string.IsNullOrEmpty(ContactProfileUsername);

    public string ContactProfileAbout
    {
        get => _contactProfileAbout;
        private set => SetProperty(ref _contactProfileAbout, value);
    }

    public bool HasContactProfileAbout => !string.IsNullOrWhiteSpace(ContactProfileAbout);

    public Bitmap? ContactProfileAvatar
    {
        get => _contactProfileAvatar;
        private set
        {
            if (SetProperty(ref _contactProfileAvatar, value)) OnPropertyChanged(nameof(HasContactProfileAvatar));
        }
    }

    public bool HasContactProfileAvatar => ContactProfileAvatar is not null;

    public bool IsOnboardingOpen
    {
        get => _isOnboardingOpen;
        private set => SetProperty(ref _isOnboardingOpen, value);
    }

    public bool IsContactProfileOpen
    {
        get => _isContactProfileOpen;
        private set => SetProperty(ref _isContactProfileOpen, value);
    }

    public bool IsLightboxOpen
    {
        get => _isLightboxOpen;
        private set => SetProperty(ref _isLightboxOpen, value);
    }

    public Bitmap? LightboxImage
    {
        get => _lightboxImage;
        private set => SetProperty(ref _lightboxImage, value);
    }

    public bool LightboxHasPrev => _lightboxIndex > 0;
    public bool LightboxHasNext => _lightboxIndex < _lightboxItems.Count - 1;

    public string NewMessage
    {
        get => _newMessage;
        set
        {
            if (SetProperty(ref _newMessage, value)) OnPropertyChanged(nameof(CanSend));
        }
    }

    public string SecurityPassphrase
    {
        get => _securityPassphrase;
        set => SetProperty(ref _securityPassphrase, value);
    }

    public string RevokeDeviceId
    {
        get => _revokeDeviceId;
        set => SetProperty(ref _revokeDeviceId, value);
    }

    public string StatusMessage
    {
        get => _statusMessage;
        private set => SetProperty(ref _statusMessage, value);
    }

    public bool IsBusy
    {
        get => _isBusy;
        private set => SetProperty(ref _isBusy, value);
    }

    public LocalContact? SelectedContact
    {
        get => _selectedContact;
        set
        {
            // Rebuilding the filtered list makes the ListBox hand back a null selection. Closing the
            // conversation is always an explicit decision, so only SelectContact(null) may do it.
            if (value is null) return;
            SelectContact(value);
        }
    }

    private void SelectContact(LocalContact? value)
    {
        if (!SetProperty(ref _selectedContact, value, nameof(SelectedContact))) return;

        OnPropertyChanged(nameof(SelectedContactTitle));
        OnPropertyChanged(nameof(HasSelectedContact));
        OnPropertyChanged(nameof(HasNoSelectedContact));
        OnPropertyChanged(nameof(HasMessages));
        OnPropertyChanged(nameof(HasNoMessages));
        OnPropertyChanged(nameof(CanSend));
        ClearMessageSelection();
        CancelEdit();
        IsContactProfileOpen = false;
        if (value is not null && IsCompact) IsChatOpen = true;
        if (value is null) IsChatOpen = false;
        OnPropertyChanged(nameof(SelectedContactPresenceOnline));
        OnPropertyChanged(nameof(SelectedContactPresenceLabel));
        OnPropertyChanged(nameof(HasSelectedContactPresence));
        _lastPresenceSentAt = DateTimeOffset.MinValue;
        if (value is { PendingApproval: false }) _ = SendPresenceHeartbeatAsync(value);
        _ = LoadMessagesAsync();
    }

    public LocalTextMessage? SelectedMessage
    {
        get => _selectedMessage;
        private set => _selectedMessage = value;
    }

    public string SelectedContactTitle => SelectedContact is null
        ? "TuratText"
        : SelectedContact.DisplayName;

    public string MetadataProtectionLabel => _metadataMode switch
    {
        MetadataProtectionMode.Balanced => "Метаданные: balanced · задержка до 2 с",
        MetadataProtectionMode.HighPrivacy => "Метаданные: high · relay-first · задержка до 15 с",
        _ => "Метаданные: fast · минимальная задержка"
    };

    public AsyncRelayCommand ConnectCommand { get; }
    public AsyncRelayCommand AddContactCommand { get; }
    public AsyncRelayCommand PublishUsernameCommand { get; }
    public AsyncRelayCommand CompleteOnboardingCommand { get; }
    public AsyncRelayCommand SaveProfileCommand { get; }
    public AsyncRelayCommand OpenContactProfileCommand { get; }
    public AsyncRelayCommand LightboxNextCommand { get; }
    public AsyncRelayCommand LightboxPrevCommand { get; }
    public RelayCommand CloseLightboxCommand { get; }
    public AsyncRelayCommand SendCommand { get; }
    public AsyncRelayCommand EditCommand { get; }
    public AsyncRelayCommand SubmitComposerCommand { get; }
    public AsyncRelayCommand DeleteCommand { get; }
    public AsyncRelayCommand ReactCommand { get; }
    public AsyncRelayCommand AcceptContactCommand { get; }
    public AsyncRelayCommand RejectContactCommand { get; }
    public AsyncRelayCommand<LocalContact> DeleteChatCommand { get; }
    public AsyncRelayCommand RefreshCommand { get; }
    public AsyncRelayCommand CheckUpdateCommand { get; }
    public AsyncRelayCommand CycleMetadataModeCommand { get; }
    public AsyncRelayCommand RevokeDeviceCommand { get; }
    public RelayCommand BackToChatsCommand { get; }
    public RelayCommand OpenNewContactCommand { get; }
    public RelayCommand OpenSettingsCommand { get; }
    public RelayCommand CloseOverlayCommand { get; }
    public RelayCommand BeginEditCommand { get; }
    public RelayCommand CancelEditCommand { get; }
    public RelayCommand ClearMessageSelectionCommand { get; }

    public event Action? MessageSelectionChanged;

    /// <summary>Raised when the conversation should scroll to the newest message.</summary>
    public event Action? ScrollToBottomRequested;

    /// <summary>Raised after the visible chat list was rebuilt, so the view can reapply the selection.</summary>
    public event Action? ContactListRebuilt;

    private void NotifyShellLayoutChanged()
    {
        OnPropertyChanged(nameof(SidebarWidth));
        OnPropertyChanged(nameof(ShowSidebar));
        OnPropertyChanged(nameof(ShowConversation));
        OnPropertyChanged(nameof(ShowBackButton));
    }

    private void ApplyContactFilter()
    {
        string query = _contactQuery.Trim();
        List<LocalContact> matches = string.IsNullOrEmpty(query)
            ? Contacts.ToList()
            : Contacts.Where(contact =>
                contact.DisplayName.Contains(query, StringComparison.CurrentCultureIgnoreCase)
                || contact.UserId.Contains(query, StringComparison.OrdinalIgnoreCase)).ToList();

        VisibleContacts.Clear();
        foreach (LocalContact contact in matches) VisibleContacts.Add(contact);

        OnPropertyChanged(nameof(HasVisibleContacts));
        OnPropertyChanged(nameof(HasNoSearchResults));

        // The list drops its selection while the rebuilt items settle. A binding will not re-push an
        // unchanged value, so ask the view to reapply it once the new items are in place.
        Dispatcher.UIThread.Post(() => ContactListRebuilt?.Invoke(), DispatcherPriority.Background);
    }

    public bool ToggleMessageSelection(LocalTextMessage message)
    {
        int existingIndex = IndexOfSelected(message);
        bool isSelected;
        if (existingIndex >= 0)
        {
            _selectedMessages.RemoveAt(existingIndex);
            isSelected = false;
        }
        else
        {
            _selectedMessages.Add(message);
            isSelected = true;
        }

        SelectedMessage = _selectedMessages.LastOrDefault();
        NotifyMessageSelectionChanged();
        return isSelected;
    }

    // Refreshing history produces new record instances, so selection is tracked by event id.
    public bool IsMessageSelected(LocalTextMessage message) => IndexOfSelected(message) >= 0;

    private int IndexOfSelected(LocalTextMessage message)
    {
        for (int index = 0; index < _selectedMessages.Count; index++)
        {
            if (_selectedMessages[index].EventId == message.EventId) return index;
        }
        return -1;
    }

    public async Task InitializeAsync()
    {
        await _runtime.InitializeAsync();
        _metadataMode = await _metadataProtection.GetAsync();
        OnPropertyChanged(nameof(MetadataProtectionLabel));
        OnPropertyChanged(nameof(UserId));
        OnPropertyChanged(nameof(DeviceId));
        OnPropertyChanged(nameof(UserIdShort));
        await LoadOwnProfileAsync();
        await LoadContactsAsync();
        await RefreshAsync();
        _ = LiveSyncLoopAsync(_pollCancellation.Token);
    }

    private async Task LoadOwnProfileAsync()
    {
        SignedProfileClaim? own = await _profiles.ReadCachedOwnProfileAsync();
        if (own is null)
        {
            IsOnboardingOpen = true;
            return;
        }
        ProfileDisplayName = own.Claim.DisplayName;
        ProfileAbout = own.Claim.About;
        ApplyAvatar(own.Claim.AvatarMimeType, own.Claim.AvatarBase64, isOwn: true);
    }

    private void ApplyAvatar(string? mimeType, string? base64, bool isOwn)
    {
        if (string.IsNullOrEmpty(base64))
        {
            if (isOwn) ProfileAvatarPreview = null; else ContactProfileAvatar = null;
            return;
        }
        try
        {
            using var stream = new MemoryStream(Convert.FromBase64String(base64));
            var bitmap = new Bitmap(stream);
            if (isOwn)
            {
                _profileAvatarMimeType = mimeType;
                _profileAvatarBase64 = base64;
                ProfileAvatarPreview = bitmap;
            }
            else
            {
                ContactProfileAvatar = bitmap;
            }
        }
        catch
        {
            // Corrupt or unreadable cached avatar: fall back to initials, no need to surface an error.
        }
    }

    private async Task ConnectAsync()
    {
        if (!Uri.TryCreate(BootstrapUrl.Trim(), UriKind.Absolute, out Uri? uri))
        {
            StatusMessage = "Введите корректный URL Mailbox Node.";
            return;
        }
        IsBusy = true;
        try
        {
            IReadOnlyList<OwnedMailboxRoute> routes = await _provisioning.ProvisionAsync([(uri, null)]);
            OwnedMailboxRoute connected = routes[^1];
            await _bootstrapNodes.AddPinnedAsync(connected.Node);
            StatusMessage = $"Подключён узел {connected.Node.Name}. Mailbox зарегистрирован и routing опубликован.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось подключить узел: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task CheckUpdateAsync()
    {
        if (!Uri.TryCreate(BootstrapUrl.TrimEnd('/') + "/updates/stable/manifest.json", UriKind.Absolute, out Uri? uri))
        {
            StatusMessage = "Введите корректный URL узла для проверки обновлений.";
            return;
        }
        IsBusy = true;
        try
        {
            VerifiedUpdate update = await _updates.FetchAndPinAsync(uri, ReleaseTrustRoot.Current);
            StatusMessage = $"Проверено threshold-подписями 2/3: TuratText {update.Manifest.Body.ReleaseVersion} "
                            + $"(sequence {update.Manifest.Body.Sequence}).";
        }
        catch (Exception exception)
        {
            StatusMessage = "Безопасное обновление не найдено или отклонено: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task CycleMetadataModeAsync()
    {
        _metadataMode = _metadataMode switch
        {
            MetadataProtectionMode.Fast => MetadataProtectionMode.Balanced,
            MetadataProtectionMode.Balanced => MetadataProtectionMode.HighPrivacy,
            _ => MetadataProtectionMode.Fast
        };
        await _metadataProtection.SetAsync(_metadataMode);
        OnPropertyChanged(nameof(MetadataProtectionLabel));
        StatusMessage = MetadataProtectionLabel + ". Padding и рандомизированные envelopes включены во всех режимах.";
    }

    private async Task RevokeDeviceAsync()
    {
        if (string.IsNullOrWhiteSpace(RevokeDeviceId)) return;
        IsBusy = true;
        try
        {
            string revoked = RevokeDeviceId.Trim();
            await _deviceLink.RevokeDeviceAsync(revoked, "revoked by identity authority");
            IReadOnlyList<OwnedMailboxRoute> routes = await _mailboxes.LoadAsync();
            if (routes.Count > 0)
            {
                await _provisioning.ProvisionAsync(routes.Select(route =>
                    (new Uri(route.Node.BaseUrl), (string?)route.Node.NodeId)));
            }
            RevokeDeviceId = "";
            StatusMessage = $"DeviceID {ShortId(revoked)} отозван; подписанный DeviceList и routing обновлены.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось отозвать устройство: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task AddContactAsync()
    {
        string lookup = NewContactUserId.Trim().ToLowerInvariant();
        IReadOnlyList<OwnedMailboxRoute> ownRoutes = await _mailboxes.LoadAsync();
        if (ownRoutes.Count == 0)
        {
            StatusMessage = "Сначала подключите Mailbox Node.";
            return;
        }
        IsBusy = true;
        try
        {
            string userId = lookup;
            if (!userId.StartsWith("tt1-", StringComparison.Ordinal) || userId.Length != 68)
            {
                IReadOnlyList<SignedUsernameClaim> claims = await _usernames.ResolveAsync(
                    ownRoutes.Select(value => value.Node),
                    lookup);
                if (claims.Count == 0) throw new InvalidOperationException("Username не найден на доступных узлах");
                if (claims.Count > 1)
                {
                    string conflicts = string.Join(", ", claims.Select(value => ShortId(value.Claim.UserId)));
                    throw new InvalidOperationException(
                        $"Username конфликтует между разделами сети ({conflicts}). Выберите полный UserID");
                }
                userId = claims[0].Claim.UserId;
            }

            SignedRoutingDescriptor? descriptor = null;
            Exception? lastError = null;
            foreach (OwnedMailboxRoute ownRoute in ownRoutes)
            {
                try
                {
                    descriptor = await _routing.GetAsync(ownRoute.Node, userId);
                    break;
                }
                catch (Exception exception)
                {
                    lastError = exception;
                }
            }
            if (descriptor is null) throw new HttpRequestException("Подписанный маршрут не найден", lastError);
            string displayName = ShortId(userId);
            try
            {
                SignedProfileClaim? profile = await _profiles.ResolveAsync(ownRoutes.Select(value => value.Node), userId);
                if (profile is { Claim.DisplayName.Length: > 0 }) displayName = profile.Claim.DisplayName;
            }
            catch
            {
                // No published profile yet, or the directory is unreachable: fall back to the short UserID.
            }
            var contact = new LocalContact(
                userId,
                displayName,
                descriptor,
                DateTimeOffset.UtcNow,
                false);
            await _contactStore.SaveAsync(contact);
            NewContactUserId = "";
            await LoadContactsAsync();
            SelectContact(Contacts.First(value => value.UserId == userId));
            IsNewContactOpen = false;
            StatusMessage = "Контакт добавлен. Сверьте fingerprint по доверенному каналу.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось получить подписанный маршрут: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task PublishUsernameAsync()
    {
        IReadOnlyList<OwnedMailboxRoute> ownRoutes = await _mailboxes.LoadAsync();
        if (ownRoutes.Count == 0)
        {
            StatusMessage = "Сначала подключите Mailbox Node.";
            return;
        }
        IsBusy = true;
        try
        {
            SignedUsernameClaim claim = await _usernames.PublishAsync(
                ownRoutes.Select(value => value.Node),
                MyUsername);
            MyUsername = claim.Claim.Username;
            StatusMessage = $"Username @{claim.Claim.Username} подписан identity и опубликован как изменяемый указатель.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось опубликовать username: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    /// <summary>Called by the view once a picked image has been downscaled into a small PNG.</summary>
    public void SetPendingAvatar(byte[] avatarPngBytes)
    {
        _profileAvatarMimeType = "image/png";
        _profileAvatarBase64 = Convert.ToBase64String(avatarPngBytes);
        using var stream = new MemoryStream(avatarPngBytes);
        ProfileAvatarPreview = new Bitmap(stream);
        StatusMessage = "Аватар выбран. Сохраните профиль, чтобы опубликовать его.";
    }

    public void ReportAvatarError(string message) => StatusMessage = message;

    private async Task CompleteOnboardingAsync()
    {
        if (string.IsNullOrWhiteSpace(MyUsername) || string.IsNullOrWhiteSpace(ProfileDisplayName))
        {
            StatusMessage = "Укажите username и видимое имя.";
            return;
        }
        IReadOnlyList<OwnedMailboxRoute> ownRoutes = await _mailboxes.LoadAsync();
        if (ownRoutes.Count == 0)
        {
            StatusMessage = "Сначала подключите Mailbox Node.";
            return;
        }
        IsBusy = true;
        try
        {
            SignedUsernameClaim usernameClaim = await _usernames.PublishAsync(
                ownRoutes.Select(value => value.Node), MyUsername);
            MyUsername = usernameClaim.Claim.Username;
            SignedProfileClaim profileClaim = await _profiles.PublishAsync(
                ownRoutes.Select(value => value.Node),
                ProfileDisplayName, ProfileAbout, _profileAvatarMimeType, _profileAvatarBase64);
            ProfileDisplayName = profileClaim.Claim.DisplayName;
            IsOnboardingOpen = false;
            StatusMessage = "Профиль опубликован. Добро пожаловать в TuratText!";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось завершить настройку профиля: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task SaveProfileAsync()
    {
        if (string.IsNullOrWhiteSpace(ProfileDisplayName))
        {
            StatusMessage = "Укажите видимое имя.";
            return;
        }
        IReadOnlyList<OwnedMailboxRoute> ownRoutes = await _mailboxes.LoadAsync();
        if (ownRoutes.Count == 0)
        {
            StatusMessage = "Сначала подключите Mailbox Node.";
            return;
        }
        IsBusy = true;
        try
        {
            SignedProfileClaim claim = await _profiles.PublishAsync(
                ownRoutes.Select(value => value.Node),
                ProfileDisplayName, ProfileAbout, _profileAvatarMimeType, _profileAvatarBase64);
            ProfileDisplayName = claim.Claim.DisplayName;
            ProfileAbout = claim.Claim.About;
            StatusMessage = "Профиль обновлён и подписан identity.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось сохранить профиль: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task OpenContactProfileAsync()
    {
        if (SelectedContact is not { } contact) return;
        ContactProfileUsername = "";
        ContactProfileAbout = "";
        ContactProfileAvatar = null;
        IsContactProfileOpen = true;
        OnPropertyChanged(nameof(HasContactProfileUsername));
        OnPropertyChanged(nameof(HasContactProfileAbout));
        OnPropertyChanged(nameof(HasContactProfileAvatar));
        try
        {
            IReadOnlyList<OwnedMailboxRoute> ownRoutes = await _mailboxes.LoadAsync();
            if (ownRoutes.Count == 0) return;
            IEnumerable<NodeDescriptor> nodes = ownRoutes.Select(value => value.Node);

            SignedUsernameClaim? username = await _usernames.ResolveByUserIdAsync(nodes, contact.UserId);
            if (username is not null) ContactProfileUsername = "@" + username.Claim.Username;

            SignedProfileClaim? profile = await _profiles.ResolveAsync(nodes, contact.UserId);
            if (profile is not null)
            {
                ContactProfileAbout = profile.Claim.About;
                ApplyAvatar(profile.Claim.AvatarMimeType, profile.Claim.AvatarBase64, isOwn: false);
            }
        }
        catch
        {
            // Best-effort enrichment: the overlay still shows the locally known contact either way.
        }
        finally
        {
            OnPropertyChanged(nameof(HasContactProfileUsername));
            OnPropertyChanged(nameof(HasContactProfileAbout));
            OnPropertyChanged(nameof(HasContactProfileAvatar));
        }
    }

    public async Task OpenLightboxAsync(MessageRow row)
    {
        if (SelectedContact is null || !row.IsImageAttachment) return;
        IReadOnlyList<LocalTextMessage> history = await _messaging.ReadConversationAsync(SelectedContact.UserId);
        _lightboxItems = history.Where(IsImageMessage).ToList();
        _lightboxIndex = _lightboxItems.FindIndex(message => message.EventId == row.Message.EventId);
        if (_lightboxIndex < 0)
        {
            _lightboxItems = [row.Message];
            _lightboxIndex = 0;
        }
        IsLightboxOpen = true;
        await ShowLightboxImageAsync();
    }

    private static bool IsImageMessage(LocalTextMessage message) =>
        !message.Deleted
        && message.Attachment?.MimeType.StartsWith("image/", StringComparison.OrdinalIgnoreCase) == true;

    private async Task ShowLightboxImageAsync()
    {
        if (_lightboxIndex < 0 || _lightboxIndex >= _lightboxItems.Count)
        {
            CloseLightbox();
            return;
        }
        LocalTextMessage message = _lightboxItems[_lightboxIndex];
        OnPropertyChanged(nameof(LightboxHasPrev));
        OnPropertyChanged(nameof(LightboxHasNext));
        if (_decryptedImageCache.TryGetValue(message.EventId, out Bitmap? cached))
        {
            LightboxImage = cached;
            return;
        }
        try
        {
            byte[] bytes = await _messaging.DownloadAttachmentAsync(message.Attachment!);
            using var stream = new MemoryStream(bytes);
            var bitmap = new Bitmap(stream);
            _decryptedImageCache[message.EventId] = bitmap;
            LightboxImage = bitmap;
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось загрузить изображение: " + exception.Message;
        }
    }

    private async Task LightboxNextAsync()
    {
        if (_lightboxIndex >= _lightboxItems.Count - 1) return;
        _lightboxIndex++;
        await ShowLightboxImageAsync();
    }

    private async Task LightboxPrevAsync()
    {
        if (_lightboxIndex <= 0) return;
        _lightboxIndex--;
        await ShowLightboxImageAsync();
    }

    private void CloseLightbox()
    {
        IsLightboxOpen = false;
        LightboxImage = null;
        _lightboxItems = [];
        _lightboxIndex = 0;
    }

    /// <summary>
    /// Kicks off best-effort background decryption for any image attachment in the given history that
    /// is not cached yet, refreshing the message list once each thumbnail is ready.
    /// </summary>
    private void QueueThumbnails(IReadOnlyList<LocalTextMessage> history)
    {
        foreach (LocalTextMessage message in history)
        {
            if (!IsImageMessage(message)) continue;
            if (_decryptedImageCache.ContainsKey(message.EventId)) continue;
            if (!_thumbnailsLoading.Add(message.EventId)) continue;
            _ = LoadThumbnailAsync(message);
        }
    }

    private async Task LoadThumbnailAsync(LocalTextMessage message)
    {
        try
        {
            byte[] bytes = await _messaging.DownloadAttachmentAsync(message.Attachment!);
            // Anything much larger is not worth decoding just to shrink it back down for a bubble.
            if (bytes.Length > 20 * 1024 * 1024) return;
            using var stream = new MemoryStream(bytes);
            var bitmap = new Bitmap(stream);
            await Dispatcher.UIThread.InvokeAsync(() =>
            {
                _decryptedImageCache[message.EventId] = bitmap;
                RefreshRowsFromCachedHistory();
            });
        }
        catch
        {
            // Leave it as the generic file chip; the user can still download and open it manually.
        }
        finally
        {
            _thumbnailsLoading.Remove(message.EventId);
        }
    }

    private async Task SendAsync()
    {
        if (SelectedContact is null || string.IsNullOrWhiteSpace(NewMessage)) return;
        string text = NewMessage;
        NewMessage = "";
        IsBusy = true;
        try
        {
            await _messaging.SendTextAsync(SelectedContact, text);
            await LoadMessagesAsync();
            StatusMessage = "Сообщение сохранено локально и передано доступным Mailbox Nodes.";
        }
        catch (Exception exception)
        {
            NewMessage = text;
            StatusMessage = "Сообщение осталось в локальном outbox: " + exception.Message;
            await LoadMessagesAsync();
        }
        finally
        {
            IsBusy = false;
        }
    }

    private Task SubmitComposerAsync() => IsEditing ? EditAsync() : SendAsync();

    private async Task EditAsync()
    {
        LocalTextMessage? target = _editingMessage ?? SelectedMessage;
        if (SelectedContact is null || target is null || !target.Outgoing
            || target.Deleted || string.IsNullOrWhiteSpace(NewMessage)) return;
        string replacement = NewMessage;
        NewMessage = "";
        _editingMessage = null;
        OnPropertyChanged(nameof(IsEditing));
        OnPropertyChanged(nameof(EditingPreview));
        await RunMessageActionAsync(
            () => _messaging.EditMessageAsync(SelectedContact, target.EventId, replacement),
            "Изменение сохранено как подписанное событие.");
    }

    private async Task DeleteAsync()
    {
        if (SelectedContact is not { } contact) return;
        IReadOnlyList<LocalTextMessage> targets = DeletableSelection;
        if (targets.Count == 0) return;

        await RunMessageActionAsync(
            contact,
            targets,
            (peer, message) => _messaging.DeleteMessageAsync(peer, message.EventId),
            count => count == 1
                ? "Сообщение удалено безвозвратно."
                : $"Удалено сообщений: {count}.");
    }

    private async Task ReactAsync()
    {
        if (SelectedContact is not { } contact) return;
        IReadOnlyList<LocalTextMessage> targets = ReactableSelection;
        if (targets.Count == 0) return;

        await RunMessageActionAsync(
            contact,
            targets,
            (peer, message) => _messaging.SetReactionAsync(peer, message.EventId, "👍", true),
            count => count == 1 ? "Реакция отправлена." : $"Реакция отправлена на {count} сообщений.");
    }

    private async Task AcceptContactAsync()
    {
        if (SelectedContact is not { PendingApproval: true } contact) return;
        await _contactStore.SetPendingApprovalAsync(contact.UserId, false);
        await LoadContactsAsync();
        SelectContact(Contacts.FirstOrDefault(value => value.UserId == contact.UserId));
        StatusMessage = "Запрос принят. Для следующих сообщений используются приватные capabilities из E2EE-обмена.";
    }

    private async Task RejectContactAsync()
    {
        if (SelectedContact is not { PendingApproval: true } contact) return;
        await _contactStore.RemoveAsync(contact.UserId);
        await LoadContactsAsync();
        StatusMessage = "Запрос контакта отклонён.";
    }

    /// <summary>
    /// Removes the contact and permanently deletes the conversation's local history. The event log
    /// is append-only for delivered protocol events, so this purges this device's copy only — it does
    /// not (and cannot) reach into the peer's own local history.
    /// </summary>
    private async Task DeleteChatAsync(LocalContact? contact)
    {
        if (contact is null) return;
        bool wasSelected = _currentHistoryContactId == contact.UserId;
        List<LocalTextMessage> staleHistory = wasSelected ? _currentHistory : [];
        if (SelectedContact?.UserId == contact.UserId) SelectContact(null);

        string conversationId = V2MessagingService.ConversationId(_identity.Current!.UserId, contact.UserId);
        await _contactStore.RemoveAsync(contact.UserId);
        await _runtime.Events.DeleteConversationAsync(conversationId);

        foreach (LocalTextMessage message in staleHistory)
        {
            _decryptedImageCache.Remove(message.EventId);
        }

        await LoadContactsAsync();
        StatusMessage = $"Чат с {contact.DisplayName} удалён на этом устройстве.";
    }

    /// <summary>
    /// Reads the picked file through its <see cref="Stream"/> rather than a local path: Android's
    /// storage picker hands back content:// URIs with no real filesystem path, so a stream is the only
    /// thing that reliably works on both desktop and mobile.
    /// </summary>
    public async Task SendAttachmentStreamAsync(Stream stream, string fileName, long length)
    {
        if (SelectedContact is null) return;
        IsBusy = true;
        try
        {
            if (length > 100L * 1024 * 1024) throw new InvalidOperationException("Максимальный размер файла — 100 МБ");
            using var buffer = new MemoryStream();
            await stream.CopyToAsync(buffer);
            await _messaging.SendAttachmentAsync(
                SelectedContact,
                buffer.ToArray(),
                fileName,
                MimeTypeFor(Path.GetExtension(fileName)));
            await LoadMessagesAsync();
            StatusMessage = "Вложение зашифровано чанками, реплицировано и отправлено внутри E2EE manifest.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось отправить вложение: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    public async Task<byte[]?> DownloadSelectedAttachmentAsync()
    {
        if (SelectedMessage?.Attachment is null) return null;
        IsBusy = true;
        try
        {
            byte[] value = await _messaging.DownloadAttachmentAsync(SelectedMessage.Attachment);
            StatusMessage = "Вложение скачано и проверено по SHA-256.";
            return value;
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось скачать вложение: " + exception.Message;
            return null;
        }
        finally
        {
            IsBusy = false;
        }
    }

    public async Task CreateBackupAsync(string path)
    {
        IsBusy = true;
        try
        {
            await _backup.CreateAsync(path, SecurityPassphrase);
            StatusMessage = "Клиентски зашифрованная резервная копия создана.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось создать резервную копию: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    public async Task RestoreBackupAsync(string path)
    {
        IsBusy = true;
        try
        {
            await _backup.RestoreAsync(path, SecurityPassphrase);
            StatusMessage = "Резервная копия восстановлена. Перезапустите приложение до продолжения работы.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось восстановить резервную копию: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    public async Task CreateDeviceLinkAsync(string path)
    {
        IsBusy = true;
        try
        {
            byte[] package = await _deviceLink.CreatePackageAsync(SecurityPassphrase);
            await File.WriteAllBytesAsync(path, package);
            StatusMessage = "Пакет связывания создан на 15 минут. Передайте его новому устройству отдельно от пароля.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось создать пакет связывания: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    public async Task ImportDeviceLinkAsync(string path)
    {
        IsBusy = true;
        try
        {
            await _deviceLink.ImportPackageAsync(
                await File.ReadAllBytesAsync(path),
                SecurityPassphrase,
                replaceExistingInstallation: true);
            StatusMessage = "Устройство связано: UserID сохранён, создан новый DeviceID. Перезапустите приложение.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось связать устройство: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    public async Task ExportPortableOutboxAsync(string path)
    {
        try
        {
            int count = await _portableEnvelopes.ExportDueOutboxAsync(path);
            StatusMessage = count == 0
                ? "В outbox нет готовых к переносу envelopes."
                : $"Экспортировано envelopes: {count}. Файл можно физически передать сетевому курьеру.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось экспортировать envelopes: " + exception.Message;
        }
    }

    public async Task ForwardPortableBundleAsync(string path)
    {
        try
        {
            int count = await _portableEnvelopes.ForwardAsync(path);
            StatusMessage = $"Переносимый bundle доставлен на Mailbox Nodes. Envelopes: {count}.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось доставить переносимый bundle: " + exception.Message;
        }
    }

    public async Task ExportDiscoveryBundleAsync(string path)
    {
        try
        {
            await File.WriteAllBytesAsync(path, await _discovery.ExportAsync());
            StatusMessage = "Подписанный social-bridge bundle с доступными Nodes и Relays экспортирован.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось экспортировать discovery bundle: " + exception.Message;
        }
    }

    public async Task ImportDiscoveryBundleAsync(string path)
    {
        try
        {
            (int nodes, int relays) = await _discovery.ImportAsync(await File.ReadAllBytesAsync(path));
            StatusMessage = $"Discovery bundle проверен и импортирован: Nodes {nodes}, Relays {relays}.";
        }
        catch (Exception exception)
        {
            StatusMessage = "Не удалось импортировать discovery bundle: " + exception.Message;
        }
    }

    /// <summary>
    /// Applies one action to every message in the current selection, then drops the selection so the
    /// action bar cannot linger over messages the user can no longer see as selected.
    /// </summary>
    private async Task RunMessageActionAsync(
        LocalContact contact,
        IReadOnlyList<LocalTextMessage> targets,
        Func<LocalContact, LocalTextMessage, Task<SignedProtocolEvent>> action,
        Func<int, string> success)
    {
        IsBusy = true;
        int applied = 0;
        try
        {
            foreach (LocalTextMessage target in targets)
            {
                await action(contact, target);
                applied++;
            }
            StatusMessage = success(applied);
        }
        catch (Exception exception)
        {
            StatusMessage = applied == 0
                ? "Операция осталась в outbox: " + exception.Message
                : $"Выполнено: {applied} из {targets.Count}. Остальное осталось в outbox: {exception.Message}";
        }
        finally
        {
            ClearMessageSelection();
            await LoadMessagesAsync();
            IsBusy = false;
        }
    }

    private async Task RunMessageActionAsync(
        Func<Task<SignedProtocolEvent>> action,
        string success)
    {
        IsBusy = true;
        try
        {
            await action();
            ClearMessageSelection();
            await LoadMessagesAsync();
            StatusMessage = success;
        }
        catch (Exception exception)
        {
            StatusMessage = "Операция осталась в outbox: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
        }
    }

    private async Task RefreshAsync()
    {
        if (IsBusy) return;
        // Serialise against the live-sync loop so both never drain the same mailbox at once.
        await _syncGate.WaitAsync(_pollCancellation.Token);
        IsBusy = true;
        try
        {
            _lastMaintenance = DateTimeOffset.UtcNow;
            await EnsureDefaultNodeAsync();
            await _messaging.FlushOutboxAsync();
            int received = await _messaging.FetchAsync();
            if (received > 0) await LoadContactsAsync();
            await LoadMessagesAsync();
            IReadOnlyList<OwnedMailboxRoute> routes = await _mailboxes.LoadAsync();
            IsOnline = routes.Count > 0;
            StatusMessage = routes.Count == 0
                ? "Офлайн: identity и история доступны, Mailbox Node ещё не подключён."
                : received > 0
                    ? $"Получено новых сообщений: {received}. Активных mailboxes: {routes.Count}."
                    : $"Синхронизация завершена. Активных mailboxes: {routes.Count}.";
        }
        catch (Exception exception)
        {
            IsOnline = false;
            StatusMessage = "Офлайн-режим: " + exception.Message;
        }
        finally
        {
            IsBusy = false;
            _syncGate.Release();
        }
    }

    private async Task EnsureDefaultNodeAsync()
    {
        IReadOnlyList<OwnedMailboxRoute> routes = await _mailboxes.LoadAsync();
        bool needsProvisioning = routes.Count == 0
                                 || routes.All(route => route.ExpiresAt <= DateTimeOffset.UtcNow.AddDays(1));
        if (!needsProvisioning) return;

        IReadOnlyList<(Uri Uri, string? ExpectedNodeId)> nodes = await _bootstrapNodes.LoadAsync();
        if (nodes.Count == 0) return;
        IReadOnlyList<OwnedMailboxRoute> provisioned = await _provisioning.ProvisionAsync(nodes);
        foreach (OwnedMailboxRoute route in provisioned) await _bootstrapNodes.AddPinnedAsync(route.Node);
    }

    private async Task LoadContactsAsync()
    {
        string? selected = SelectedContact?.UserId;
        IReadOnlyList<LocalContact> contacts = await _messaging.ContactsAsync();

        Contacts.Clear();
        foreach (LocalContact contact in contacts.OrderBy(value => value.DisplayName)) Contacts.Add(contact);

        OnPropertyChanged(nameof(HasContacts));
        OnPropertyChanged(nameof(HasNoContacts));
        ApplyContactFilter();

        LocalContact? next = Contacts.FirstOrDefault(value => value.UserId == selected)
                             ?? (IsWide && !IsMobile ? Contacts.FirstOrDefault() : null);
        if (next is not null && next.UserId == _selectedContact?.UserId)
        {
            // Same conversation, refreshed record: swap the instance quietly. Re-selecting would drop
            // the message selection and reload the history on every arriving message.
            _selectedContact = next;
            OnPropertyChanged(nameof(SelectedContact));
            OnPropertyChanged(nameof(SelectedContactTitle));
            return;
        }
        SelectContact(next);
    }

    private async Task LoadMessagesAsync()
    {
        if (SelectedContact is null)
        {
            MessageRows.Clear();
            ClearMessageSelection();
            OnPropertyChanged(nameof(HasMessages));
            OnPropertyChanged(nameof(HasNoMessages));
            return;
        }

        List<LocalTextMessage> history = (await _messaging.ReadConversationAsync(SelectedContact.UserId)).ToList();
        _currentHistory = history;
        _currentHistoryContactId = SelectedContact.UserId;
        PruneImageCache(history);
        SyncMessageRows(BuildRows(history, _decryptedImageCache));
        QueueThumbnails(history);
        OnPropertyChanged(nameof(HasMessages));
        OnPropertyChanged(nameof(HasNoMessages));
    }

    /// <summary>Drops cached thumbnails for messages that are no longer part of the conversation
    /// (e.g. deleted), so memory doesn't hold decrypted images for rows that can't be shown anymore.</summary>
    private void PruneImageCache(IReadOnlyList<LocalTextMessage> history)
    {
        if (_decryptedImageCache.Count == 0) return;
        var live = new HashSet<string>(history.Select(value => value.EventId), StringComparer.Ordinal);
        foreach (string eventId in _decryptedImageCache.Keys.Where(key => !live.Contains(key)).ToList())
        {
            _decryptedImageCache.Remove(eventId);
        }
    }

    /// <summary>
    /// Re-renders the currently displayed rows from the in-memory history snapshot instead of
    /// replaying the whole conversation from the event log again. Used when a thumbnail finishes
    /// decoding, since only that one row's image actually changed.
    /// </summary>
    private void RefreshRowsFromCachedHistory()
    {
        if (SelectedContact is null || _currentHistoryContactId != SelectedContact.UserId) return;
        SyncMessageRows(BuildRows(_currentHistory, _decryptedImageCache));
    }

    /// <summary>
    /// Turns a flat history into display rows, adding day separators and grouping consecutive
    /// messages from the same side so bubbles can be drawn as a single visual block.
    /// </summary>
    private static List<MessageRow> BuildRows(
        IReadOnlyList<LocalTextMessage> history,
        IReadOnlyDictionary<string, Bitmap> imageCache)
    {
        var rows = new List<MessageRow>(history.Count);
        for (int index = 0; index < history.Count; index++)
        {
            LocalTextMessage message = history[index];
            LocalTextMessage? previous = index > 0 ? history[index - 1] : null;
            LocalTextMessage? next = index < history.Count - 1 ? history[index + 1] : null;

            bool newDay = previous is null
                          || previous.CreatedAt.ToLocalTime().Date != message.CreatedAt.ToLocalTime().Date;
            bool groupStart = newDay
                              || previous!.Outgoing != message.Outgoing
                              || message.CreatedAt - previous.CreatedAt > TimeSpan.FromMinutes(5);
            bool groupEnd = next is null
                            || next.Outgoing != message.Outgoing
                            || next.CreatedAt.ToLocalTime().Date != message.CreatedAt.ToLocalTime().Date
                            || next.CreatedAt - message.CreatedAt > TimeSpan.FromMinutes(5);

            imageCache.TryGetValue(message.EventId, out Bitmap? thumbnail);
            rows.Add(new MessageRow(message, newDay, groupStart, groupEnd, thumbnail));
        }
        return rows;
    }

    /// <summary>
    /// Patches the bound collection in place. Refreshing every 15 seconds must not replay the entry
    /// animation for the whole conversation or throw the reader back to the top of the scroll.
    /// </summary>
    private void SyncMessageRows(List<MessageRow> incoming)
    {
        int index = 0;
        while (index < MessageRows.Count
               && index < incoming.Count
               && MessageRows[index].Signature == incoming[index].Signature)
        {
            index++;
        }

        int appendedFrom = MessageRows.Count;
        for (int cursor = index; cursor < MessageRows.Count && cursor < incoming.Count; cursor++)
        {
            MessageRows[cursor] = incoming[cursor];
        }
        while (MessageRows.Count > incoming.Count) MessageRows.RemoveAt(MessageRows.Count - 1);
        while (MessageRows.Count < incoming.Count) MessageRows.Add(incoming[MessageRows.Count]);

        if (incoming.Count > appendedFrom || index < appendedFrom) ScrollToBottomRequested?.Invoke();
    }

    /// <summary>
    /// Keeps the conversation live. The loop only flushes the outbox and drains the mailbox, which is
    /// a single cheap request, so it can run about once a second while the app is in use; the costly
    /// maintenance work (node provisioning, mailbox status) is folded in once a minute. History is
    /// re-read only when something actually arrived, so an idle chat costs nothing on the UI thread.
    /// </summary>
    private async Task LiveSyncLoopAsync(CancellationToken cancellationToken)
    {
        while (!cancellationToken.IsCancellationRequested)
        {
            try
            {
                await Task.Delay(NextSyncDelay(), cancellationToken);
                await LiveSyncAsync(cancellationToken);
            }
            catch (OperationCanceledException)
            {
                return;
            }
            catch (Exception exception)
            {
                System.Diagnostics.Debug.WriteLine($"Live sync tick failed: {exception}");
            }
        }
    }

    private TimeSpan NextSyncDelay()
    {
        if (!IsSurfaceActive) return TimeSpan.FromSeconds(10);
        return DateTimeOffset.UtcNow - _lastInteraction < IdleAfter
            ? ActiveSyncInterval
            : TimeSpan.FromSeconds(5);
    }

    /// <summary>
    /// Runs off the UI thread: decryption, SQLite and HTTP must never share the render thread at this
    /// cadence. Only the resulting state changes are marshalled back.
    /// </summary>
    private async Task LiveSyncAsync(CancellationToken cancellationToken)
    {
        if (!await _syncGate.WaitAsync(0, cancellationToken)) return;
        try
        {
            if (DateTimeOffset.UtcNow - _lastMaintenance > TimeSpan.FromSeconds(60))
            {
                _lastMaintenance = DateTimeOffset.UtcNow;
                await EnsureDefaultNodeAsync();
                bool online = (await _mailboxes.LoadAsync(cancellationToken)).Count > 0;
                await Dispatcher.UIThread.InvokeAsync(() => IsOnline = online);
            }

            await _messaging.FlushOutboxAsync(cancellationToken);
            int received = await _messaging.FetchAsync(cancellationToken);
            if (received > 0)
            {
                await Dispatcher.UIThread.InvokeAsync(async () =>
                {
                    IsOnline = true;
                    await LoadContactsAsync();
                    await LoadMessagesAsync();
                });
            }

            await Dispatcher.UIThread.InvokeAsync(() =>
            {
                if (SelectedContact is { PendingApproval: false } contact
                    && DateTimeOffset.UtcNow - _lastPresenceSentAt > PresenceHeartbeatInterval)
                {
                    _ = SendPresenceHeartbeatAsync(contact);
                }
            });
        }
        catch (OperationCanceledException)
        {
            throw;
        }
        catch (Exception exception)
        {
            await Dispatcher.UIThread.InvokeAsync(() =>
            {
                IsOnline = false;
                StatusMessage = "Офлайн-режим: " + exception.Message;
            });
        }
        finally
        {
            _syncGate.Release();
        }
    }

    /// <summary>Called by the view once the selection has been placed on the clipboard.</summary>
    public void ReportCopiedSelection()
    {
        int count = _selectedMessages.Count;
        StatusMessage = count == 1 ? "Сообщение скопировано." : $"Скопировано сообщений: {count}.";
        ClearMessageSelection();
    }

    /// <summary>Called by the view on input so an idle client can back off without feeling stale.</summary>
    public void NotifyUserActivity() => _lastInteraction = DateTimeOffset.UtcNow;

    /// <summary>Set by the view when the window gains or loses focus.</summary>
    public void SetSurfaceActive(bool active)
    {
        IsSurfaceActive = active;
        if (active) NotifyUserActivity();
    }

    public bool IsSurfaceActive { get; private set; } = true;

    private void BackToChats()
    {
        IsChatOpen = false;
        SelectContact(null);
    }

    private void ClearMessageSelection()
    {
        if (_selectedMessages.Count == 0 && SelectedMessage is null) return;
        _selectedMessages.Clear();
        SelectedMessage = null;
        NotifyMessageSelectionChanged();
    }

    private void NotifyMessageSelectionChanged()
    {
        OnPropertyChanged(nameof(SelectedMessage));
        OnPropertyChanged(nameof(HasSelectedMessage));
        OnPropertyChanged(nameof(HasSelectedMessages));
        OnPropertyChanged(nameof(HasSingleSelectedMessage));
        OnPropertyChanged(nameof(SelectedMessageCountLabel));
        OnPropertyChanged(nameof(HasSelectedAttachment));
        OnPropertyChanged(nameof(CanModifySelectedMessage));
        OnPropertyChanged(nameof(CanDeleteSelectedMessages));
        OnPropertyChanged(nameof(CanReactSelectedMessage));
        OnPropertyChanged(nameof(CanCopySelectedMessages));
        OnPropertyChanged(nameof(DeleteSelectionLabel));
        OnPropertyChanged(nameof(ReactSelectionLabel));
        OnPropertyChanged(nameof(CopySelectionLabel));
        OnPropertyChanged(nameof(HasSelectedActions));
        MessageSelectionChanged?.Invoke();
    }

    private void CloseOverlays()
    {
        IsNewContactOpen = false;
        IsSettingsOpen = false;
        IsContactProfileOpen = false;
        if (IsLightboxOpen) CloseLightbox();
    }

    private void BeginEdit()
    {
        if (SelectedMessage is not { Outgoing: true, Deleted: false } message) return;
        _editingMessage = message;
        NewMessage = message.Text;
        OnPropertyChanged(nameof(IsEditing));
        OnPropertyChanged(nameof(EditingPreview));
    }

    private void CancelEdit()
    {
        _editingMessage = null;
        NewMessage = "";
        OnPropertyChanged(nameof(IsEditing));
        OnPropertyChanged(nameof(EditingPreview));
    }

    private static string ShortId(string value) => value.Length <= 18 ? value : value[..10] + "…" + value[^6..];

    private static string MimeTypeFor(string extension) => extension.ToLowerInvariant() switch
    {
        ".png" => "image/png",
        ".jpg" or ".jpeg" => "image/jpeg",
        ".gif" => "image/gif",
        ".webp" => "image/webp",
        ".pdf" => "application/pdf",
        ".txt" => "text/plain",
        ".json" => "application/json",
        ".zip" => "application/zip",
        _ => "application/octet-stream"
    };
}
