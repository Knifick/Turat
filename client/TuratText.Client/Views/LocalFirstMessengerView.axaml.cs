using System.Collections.Specialized;
using System.ComponentModel;
using Avalonia;
using Avalonia.Animation;
using Avalonia.Animation.Easings;
using Avalonia.Controls;
using Avalonia.Controls.Platform;
using Avalonia.Input;
using Avalonia.Input.Platform;
using Avalonia.Interactivity;
using Avalonia.Media;
using Avalonia.Platform.Storage;
using Avalonia.Styling;
using Avalonia.Threading;
using Avalonia.VisualTree;
using TuratText.Client.Messaging.V2;
using TuratText.Client.Ui;
using TuratText.Client.ViewModels;

namespace TuratText.Client.Views;

public partial class LocalFirstMessengerView : UserControl
{
    private const double HoldThresholdPixels = 12;
    // Touch input reports jitter well beyond a mouse's, so cancelling a hold at the same 12px used
    // for mouse input makes long-press-to-select nearly impossible to trigger on a phone.
    private const double TouchHoldThresholdPixels = 24;
    private const double StickToBottomTolerance = 90;

    private readonly DispatcherTimer _messageHoldTimer = new() { Interval = TimeSpan.FromMilliseconds(450) };
    private readonly DispatcherTimer _scrollAnimationTimer = new() { Interval = TimeSpan.FromMilliseconds(16) };
    private readonly ThicknessTransition _insetTransition = new()
    {
        Property = Decorator.PaddingProperty,
        Duration = TimeSpan.FromMilliseconds(220),
        Easing = new CubicEaseOut()
    };

    private LocalFirstMessengerViewModel? _viewModel;
    private TopLevel? _topLevel;
    private IInsetsManager? _insets;
    private IInputPane? _inputPane;
    private Thickness _safeArea;
    private double _keyboardHeight;

    private LocalTextMessage? _pressedMessage;
    private Point _pressPosition;
    private bool _longPressActivated;

    private MessageRow? _pressedImageRow;
    private Point _imagePressPosition;
    private bool _imageDragged;

    private Point _lightboxPressPosition;
    private bool _lightboxTracking;

    private bool? _animatedPaneShowsConversation;
    private bool _stickToBottom = true;
    private double _scrollFrom;
    private double _scrollTo;
    private DateTime _scrollStartedAt;

    public LocalFirstMessengerView()
    {
        InitializeComponent();
        DataContextChanged += OnDataContextChanged;
        _messageHoldTimer.Tick += MessageHoldTimer_OnTick;
        _scrollAnimationTimer.Tick += ScrollAnimationTimer_OnTick;

        Shell.SizeChanged += Shell_OnSizeChanged;
        MessagesScroll.ScrollChanged += MessagesScroll_OnScrollChanged;
        InsetRoot.Transitions = new Transitions { _insetTransition };

        // Input keeps the live-sync loop in its fast cadence.
        AddHandler(PointerPressedEvent, (_, _) => _viewModel?.NotifyUserActivity(), RoutingStrategies.Tunnel);
        AddHandler(KeyDownEvent, (_, _) => _viewModel?.NotifyUserActivity(), RoutingStrategies.Tunnel);
        AddHandler(PointerWheelChangedEvent, (_, _) => _viewModel?.NotifyUserActivity(), RoutingStrategies.Tunnel);
    }

    // ---------------------------------------------------------------- lifecycle

    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);

        _topLevel = TopLevel.GetTopLevel(this);
        if (_topLevel is null) return;

        // This view owns its insets: it paints edge to edge and lifts the composer above the soft
        // keyboard itself, so the framework must not also pad the content.
        DisableAutomaticSafeAreaPadding();

        _insets = _topLevel.InsetsManager;
        if (_insets is not null)
        {
            _insets.DisplayEdgeToEdgePreference = true;
            _insets.SafeAreaChanged += Insets_OnSafeAreaChanged;
            _safeArea = _insets.SafeAreaPadding;
        }

        _inputPane = _topLevel.InputPane;
        if (_inputPane is not null)
        {
            _inputPane.StateChanged += InputPane_OnStateChanged;
            _keyboardHeight = _inputPane.State == InputPaneState.Open ? _inputPane.OccludedRect.Height : 0;
        }

        _topLevel.BackRequested += TopLevel_OnBackRequested;

        if (_topLevel is Window window)
        {
            window.Activated += Window_OnActivated;
            window.Deactivated += Window_OnDeactivated;
            _viewModel?.SetSurfaceActive(window.IsActive);
        }

        ApplyInsets(TimeSpan.Zero);
        Shell_OnSizeChanged(null, null);
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        if (_insets is not null) _insets.SafeAreaChanged -= Insets_OnSafeAreaChanged;
        if (_inputPane is not null) _inputPane.StateChanged -= InputPane_OnStateChanged;
        if (_topLevel is not null) _topLevel.BackRequested -= TopLevel_OnBackRequested;
        if (_topLevel is Window window)
        {
            window.Activated -= Window_OnActivated;
            window.Deactivated -= Window_OnDeactivated;
        }
        _insets = null;
        _inputPane = null;
        _topLevel = null;
        _scrollAnimationTimer.Stop();
        base.OnDetachedFromVisualTree(e);
    }

    private void DisableAutomaticSafeAreaPadding()
    {
        TopLevel.SetAutoSafeAreaPadding(this, false);
        foreach (Control ancestor in this.GetVisualAncestors().OfType<Control>())
        {
            TopLevel.SetAutoSafeAreaPadding(ancestor, false);
        }
    }

    private void Window_OnActivated(object? sender, EventArgs e) => _viewModel?.SetSurfaceActive(true);

    private void Window_OnDeactivated(object? sender, EventArgs e) => _viewModel?.SetSurfaceActive(false);

    /// <summary>The hardware/gesture back button unwinds the UI instead of leaving the app.</summary>
    private void TopLevel_OnBackRequested(object? sender, RoutedEventArgs e)
    {
        if (_viewModel is null) return;

        if (_viewModel.IsNewContactOpen || _viewModel.IsSettingsOpen
            || _viewModel.IsContactProfileOpen || _viewModel.IsLightboxOpen)
        {
            _viewModel.CloseOverlayCommand.Execute(null);
            e.Handled = true;
        }
        else if (_viewModel.HasSelectedMessages)
        {
            _viewModel.ClearMessageSelectionCommand.Execute(null);
            e.Handled = true;
        }
        else if (_viewModel.IsCompact && _viewModel.IsChatOpen)
        {
            _viewModel.BackToChatsCommand.Execute(null);
            e.Handled = true;
        }
    }

    // ---------------------------------------------------------------- insets and keyboard

    private void Insets_OnSafeAreaChanged(object? sender, SafeAreaChangedArgs e)
    {
        _safeArea = e.SafeAreaPadding;
        ApplyInsets(TimeSpan.FromMilliseconds(160));
    }

    private void InputPane_OnStateChanged(object? sender, InputPaneStateEventArgs e)
    {
        _keyboardHeight = e.NewState == InputPaneState.Open ? e.EndRect.Height : 0;
        ApplyInsets(e.AnimationDuration, e.Easing);

        // The composer follows the keyboard up via the padding; keep the newest message in view too.
        if (e.NewState == InputPaneState.Open && _stickToBottom)
        {
            Dispatcher.UIThread.Post(() => ScrollToBottom(animated: true), DispatcherPriority.Background);
        }
    }

    private void ApplyInsets(TimeSpan duration, IEasing? easing = null)
    {
        _insetTransition.Duration = duration <= TimeSpan.Zero ? TimeSpan.FromMilliseconds(1) : duration;
        _insetTransition.Easing = easing as Easing ?? new CubicEaseOut();
        InsetRoot.Padding = new Thickness(
            _safeArea.Left,
            _safeArea.Top,
            _safeArea.Right,
            _safeArea.Bottom + Math.Max(0, _keyboardHeight));
    }

    // ---------------------------------------------------------------- adaptive layout

    private void Shell_OnSizeChanged(object? sender, SizeChangedEventArgs? e)
    {
        if (_viewModel is null) return;
        double width = Shell.Bounds.Width > 0 ? Shell.Bounds.Width : Bounds.Width;
        if (width > 0) _viewModel.ViewportWidth = width;
    }

    private void OnDataContextChanged(object? sender, EventArgs e)
    {
        if (_viewModel is not null)
        {
            _viewModel.MessageSelectionChanged -= RefreshMessageSelectionVisuals;
            _viewModel.ScrollToBottomRequested -= OnScrollToBottomRequested;
            _viewModel.ContactListRebuilt -= OnContactListRebuilt;
            _viewModel.MessageRows.CollectionChanged -= MessageRows_OnCollectionChanged;
            _viewModel.PropertyChanged -= ViewModel_OnPropertyChanged;
        }

        _viewModel = DataContext as LocalFirstMessengerViewModel;
        if (_viewModel is not null)
        {
            _viewModel.MessageSelectionChanged += RefreshMessageSelectionVisuals;
            _viewModel.ScrollToBottomRequested += OnScrollToBottomRequested;
            _viewModel.ContactListRebuilt += OnContactListRebuilt;
            _viewModel.MessageRows.CollectionChanged += MessageRows_OnCollectionChanged;
            _viewModel.PropertyChanged += ViewModel_OnPropertyChanged;
            Shell_OnSizeChanged(null, null);
        }

        RefreshMessageSelectionVisuals();
    }

    private void ViewModel_OnPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (_viewModel is null) return;

        switch (e.PropertyName)
        {
            case nameof(LocalFirstMessengerViewModel.ShowConversation):
            case nameof(LocalFirstMessengerViewModel.ShowSidebar):
                AnimatePaneSwap();
                break;
            case nameof(LocalFirstMessengerViewModel.SelectedContact):
                _stickToBottom = true;
                Dispatcher.UIThread.Post(() => ScrollToBottom(animated: false), DispatcherPriority.Background);
                break;
        }
    }

    /// <summary>
    /// A rebuilt item source leaves the list unselected and a binding will not re-push an unchanged
    /// value, so the open conversation is reapplied to the control directly.
    /// </summary>
    private void OnContactListRebuilt()
    {
        if (_viewModel?.SelectedContact is not { } contact) return;
        if (!ReferenceEquals(ContactsList.SelectedItem, contact) && _viewModel.VisibleContacts.Contains(contact))
        {
            ContactsList.SelectedItem = contact;
        }
    }

    /// <summary>
    /// In the single-pane layout the incoming panel slides in like a native push/pop. ShowSidebar and
    /// ShowConversation always change together, so the animation is keyed on the resulting state to
    /// keep the second notification from replaying it.
    /// </summary>
    private void AnimatePaneSwap()
    {
        if (_viewModel is null || _viewModel.IsWide)
        {
            _animatedPaneShowsConversation = null;
            return;
        }

        bool showConversation = _viewModel.ShowConversation;
        if (_animatedPaneShowsConversation == showConversation) return;
        _animatedPaneShowsConversation = showConversation;

        Control target = showConversation ? ChatPane : Sidebar;
        _ = SlideIn(showConversation ? 46 : -46).RunAsync(target);
    }

    private static Animation SlideIn(double fromX) => new()
    {
        Duration = TimeSpan.FromMilliseconds(270),
        Easing = new CubicEaseOut(),
        FillMode = FillMode.Forward,
        Children =
        {
            new KeyFrame
            {
                Cue = new Cue(0d),
                Setters =
                {
                    new Setter(OpacityProperty, 0d),
                    new Setter(TranslateTransform.XProperty, fromX)
                }
            },
            new KeyFrame
            {
                Cue = new Cue(1d),
                Setters =
                {
                    new Setter(OpacityProperty, 1d),
                    new Setter(TranslateTransform.XProperty, 0d)
                }
            }
        }
    };

    // ---------------------------------------------------------------- scrolling

    private void OnScrollToBottomRequested()
    {
        if (!_stickToBottom) return;
        Dispatcher.UIThread.Post(() => ScrollToBottom(animated: true), DispatcherPriority.Background);
    }

    private void MessagesScroll_OnScrollChanged(object? sender, ScrollChangedEventArgs e)
    {
        double maximum = Math.Max(0, MessagesScroll.Extent.Height - MessagesScroll.Viewport.Height);
        bool contentChanged = Math.Abs(e.ExtentDelta.Y) > 0.5 || Math.Abs(e.ViewportDelta.Y) > 0.5;

        if (contentChanged)
        {
            // New bubbles or a shrinking viewport (keyboard) must not push the newest message away.
            if (_stickToBottom && !_scrollAnimationTimer.IsEnabled && maximum - MessagesScroll.Offset.Y > 0.5)
            {
                MessagesScroll.Offset = MessagesScroll.Offset.WithY(maximum);
            }
        }
        else if (!_scrollAnimationTimer.IsEnabled)
        {
            _stickToBottom = maximum - MessagesScroll.Offset.Y <= StickToBottomTolerance;
        }

        ScrollDownButton.IsVisible = !_stickToBottom && maximum > 8;
    }

    private void ScrollDown_OnClick(object? sender, RoutedEventArgs e)
    {
        _stickToBottom = true;
        ScrollToBottom(animated: true);
    }

    private void ScrollToBottom(bool animated)
    {
        double maximum = Math.Max(0, MessagesScroll.Extent.Height - MessagesScroll.Viewport.Height);
        if (!animated || Math.Abs(maximum - MessagesScroll.Offset.Y) < 2)
        {
            _scrollAnimationTimer.Stop();
            MessagesScroll.Offset = MessagesScroll.Offset.WithY(maximum);
            return;
        }

        _scrollFrom = MessagesScroll.Offset.Y;
        _scrollTo = maximum;
        _scrollStartedAt = DateTime.UtcNow;
        _scrollAnimationTimer.Start();
    }

    private void ScrollAnimationTimer_OnTick(object? sender, EventArgs e)
    {
        const double durationMs = 260;
        double elapsed = (DateTime.UtcNow - _scrollStartedAt).TotalMilliseconds;
        double progress = Math.Clamp(elapsed / durationMs, 0, 1);
        double eased = 1 - Math.Pow(1 - progress, 3);

        // The extent keeps growing while bubbles animate in, so re-target on every frame.
        double maximum = Math.Max(0, MessagesScroll.Extent.Height - MessagesScroll.Viewport.Height);
        _scrollTo = maximum;
        MessagesScroll.Offset = MessagesScroll.Offset.WithY(_scrollFrom + (_scrollTo - _scrollFrom) * eased);

        if (progress >= 1)
        {
            _scrollAnimationTimer.Stop();
            MessagesScroll.Offset = MessagesScroll.Offset.WithY(maximum);
        }
    }

    // ---------------------------------------------------------------- message selection

    private void Message_OnPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (sender is not Border { DataContext: MessageRow row } border) return;
        _pressedMessage = row.Message;
        _pressPosition = e.GetPosition(this);
        _longPressActivated = false;
        // Without an explicit capture, the surrounding ScrollViewer can grab the pointer for its own
        // scroll gesture on touch before the hold timer elapses, via PointerCaptureLost cancelling us
        // early. Capturing here keeps the hold alive; a real drag still cancels it via the movement
        // threshold below and releases capture so the ScrollViewer can take over.
        e.Pointer.Capture(border);
        _messageHoldTimer.Start();
    }

    private void Message_OnPointerMoved(object? sender, PointerEventArgs e)
    {
        if (_pressedMessage is null) return;
        Point current = e.GetPosition(this);
        double threshold = e.Pointer.Type == PointerType.Touch ? TouchHoldThresholdPixels : HoldThresholdPixels;
        if (Math.Abs(current.X - _pressPosition.X) > threshold
            || Math.Abs(current.Y - _pressPosition.Y) > threshold)
        {
            e.Pointer.Capture(null);
            CancelMessageHold();
        }
    }

    private void Message_OnPointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        LocalTextMessage? message = _pressedMessage;
        bool longPressActivated = _longPressActivated;
        e.Pointer.Capture(null);
        CancelMessageHold();
        if (message is null || longPressActivated || _viewModel?.HasSelectedMessages != true) return;

        _viewModel.ToggleMessageSelection(message);
        e.Handled = true;
    }

    private void Message_OnPointerCaptureLost(object? sender, PointerCaptureLostEventArgs e) => CancelMessageHold();

    private void MessageHoldTimer_OnTick(object? sender, EventArgs e)
    {
        _messageHoldTimer.Stop();
        if (_pressedMessage is null || _viewModel is null) return;

        _viewModel.ToggleMessageSelection(_pressedMessage);
        _longPressActivated = true;
    }

    private void CancelMessageHold()
    {
        _messageHoldTimer.Stop();
        _pressedMessage = null;
    }

    /// <summary>
    /// Patched rows get fresh containers without the selection class, so reapply it once the new
    /// containers exist.
    /// </summary>
    private void MessageRows_OnCollectionChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        if (_viewModel?.HasSelectedMessages != true) return;
        Dispatcher.UIThread.Post(RefreshMessageSelectionVisuals, DispatcherPriority.Background);
    }

    private void RefreshMessageSelectionVisuals()
    {
        foreach (Border row in this.GetVisualDescendants().OfType<Border>()
                     .Where(border => border.Classes.Contains("messageRow")
                                      && border.DataContext is MessageRow))
        {
            var context = (MessageRow)row.DataContext!;
            row.Classes.Set("selected", _viewModel?.IsMessageSelected(context.Message) == true);
        }
    }

    // ---------------------------------------------------------------- composer

    private void Composer_OnKeyDown(object? sender, KeyEventArgs e)
    {
        if (e.Key != Key.Enter || e.KeyModifiers.HasFlag(KeyModifiers.Shift)) return;
        if (_viewModel is null || _viewModel.IsMobile) return;
        if (!_viewModel.SubmitComposerCommand.CanExecute(null)) return;

        _viewModel.SubmitComposerCommand.Execute(null);
        e.Handled = true;
    }

    private async void CopySelection_OnClick(object? sender, RoutedEventArgs e)
    {
        if (_viewModel is null || TopLevel.GetTopLevel(this)?.Clipboard is not { } clipboard) return;
        string text = _viewModel.SelectedMessagesText;
        if (string.IsNullOrEmpty(text)) return;

        await clipboard.SetTextAsync(text);
        _viewModel.ReportCopiedSelection();
    }

    private void Scrim_OnPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (!ReferenceEquals(e.Source, sender) || _viewModel is null) return;
        _viewModel.CloseOverlayCommand.Execute(null);
    }

    // ---------------------------------------------------------------- files

    private async void PickAttachment_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel
            || TopLevel.GetTopLevel(this)?.StorageProvider is not { } storage) return;
        IReadOnlyList<IStorageFile> files = await storage.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = "Выберите вложение",
            AllowMultiple = false
        });
        IStorageFile? file = files.FirstOrDefault();
        if (file is null) return;

        // Read through the file's stream rather than TryGetLocalPath(): Android's storage picker
        // hands back content:// URIs with no real filesystem path, which silently dropped every
        // picked image before. Streaming works identically on desktop and mobile.
        var properties = await file.GetBasicPropertiesAsync();
        await using Stream stream = await file.OpenReadAsync();
        await viewModel.SendAttachmentStreamAsync(stream, file.Name, (long)(properties.Size ?? 0));
    }

    private async void PickAvatar_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel
            || TopLevel.GetTopLevel(this)?.StorageProvider is not { } storage) return;
        IReadOnlyList<IStorageFile> files = await storage.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = "Выберите аватар",
            AllowMultiple = false,
            FileTypeFilter = [FilePickerFileTypes.ImageAll]
        });
        IStorageFile? file = files.FirstOrDefault();
        if (file is null) return;

        try
        {
            byte[]? avatarPng;
            await using (Stream stream = await file.OpenReadAsync())
            {
                avatarPng = AvatarImaging.EncodeDownscaled(stream);
            }
            if (avatarPng is null)
            {
                viewModel.ReportAvatarError("Не удалось сжать изображение до нужного размера — выберите другое.");
                return;
            }
            viewModel.SetPendingAvatar(avatarPng);
        }
        catch (Exception exception)
        {
            viewModel.ReportAvatarError("Не удалось обработать изображение: " + exception.Message);
        }
    }

    private void ContactHeader_OnPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (_viewModel?.SelectedContact is null) return;
        _viewModel.OpenContactProfileCommand.Execute(null);
        e.Handled = true;
    }

    // Opening on release (rather than press) mirrors platform tap conventions and lets a drag over
    // the thumbnail scroll the list instead of always launching the viewer.
    private void ImageThumbnail_OnPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (sender is not Border { DataContext: MessageRow row } border) return;
        _pressedImageRow = row;
        _imagePressPosition = e.GetPosition(this);
        _imageDragged = false;
        e.Pointer.Capture(border);
        e.Handled = true;
    }

    private void ImageThumbnail_OnPointerMoved(object? sender, PointerEventArgs e)
    {
        if (_pressedImageRow is null) return;
        Point current = e.GetPosition(this);
        double threshold = e.Pointer.Type == PointerType.Touch ? TouchHoldThresholdPixels : HoldThresholdPixels;
        if (Math.Abs(current.X - _imagePressPosition.X) > threshold
            || Math.Abs(current.Y - _imagePressPosition.Y) > threshold)
        {
            _imageDragged = true;
            e.Pointer.Capture(null);
        }
    }

    private void ImageThumbnail_OnPointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        MessageRow? row = _pressedImageRow;
        bool dragged = _imageDragged;
        _pressedImageRow = null;
        e.Pointer.Capture(null);
        e.Handled = true;
        if (row is null || dragged || _viewModel is null) return;

        _ = _viewModel.OpenLightboxAsync(row);
    }

    private void ImageThumbnail_OnPointerCaptureLost(object? sender, PointerCaptureLostEventArgs e) =>
        _pressedImageRow = null;

    private void LightboxImage_OnPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        _lightboxPressPosition = e.GetPosition(this);
        _lightboxTracking = true;
    }

    private void LightboxImage_OnPointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        if (!_lightboxTracking || _viewModel is null) return;
        _lightboxTracking = false;

        const double swipeThreshold = 60;
        Point released = e.GetPosition(this);
        double deltaX = released.X - _lightboxPressPosition.X;
        double deltaY = released.Y - _lightboxPressPosition.Y;
        if (Math.Abs(deltaX) < swipeThreshold || Math.Abs(deltaX) < Math.Abs(deltaY)) return;

        if (deltaX < 0) _viewModel.LightboxNextCommand.Execute(null);
        else _viewModel.LightboxPrevCommand.Execute(null);
    }

    private async void DownloadAttachment_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel
            || viewModel.SelectedMessage?.Attachment is not { } attachment
            || TopLevel.GetTopLevel(this)?.StorageProvider is not { } storage) return;
        IStorageFile? destination = await storage.SaveFilePickerAsync(new FilePickerSaveOptions
        {
            Title = "Сохранить расшифрованное вложение",
            SuggestedFileName = attachment.FileName
        });
        string? path = destination?.TryGetLocalPath();
        if (path is null) return;
        byte[]? content = await viewModel.DownloadSelectedAttachmentAsync();
        if (content is not null) await File.WriteAllBytesAsync(path, content);
    }

    private async void CreateBackup_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickSavePathAsync("Создать резервную копию", "turattext.ttbackup");
        if (path is not null) await viewModel.CreateBackupAsync(path);
    }

    private async void RestoreBackup_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickOpenPathAsync("Восстановить резервную копию");
        if (path is not null) await viewModel.RestoreBackupAsync(path);
    }

    private async void CreateDeviceLink_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickSavePathAsync("Создать пакет связывания", "turattext.ttlink");
        if (path is not null) await viewModel.CreateDeviceLinkAsync(path);
    }

    private async void ImportDeviceLink_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickOpenPathAsync("Связать это устройство (локальная identity будет заменена)");
        if (path is not null) await viewModel.ImportDeviceLinkAsync(path);
    }

    private async void ExportMesh_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickSavePathAsync("Экспортировать переносимые envelopes", "outbox.ttenv");
        if (path is not null) await viewModel.ExportPortableOutboxAsync(path);
    }

    private async void ForwardMesh_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickOpenPathAsync("Доставить переносимые envelopes");
        if (path is not null) await viewModel.ForwardPortableBundleAsync(path);
    }

    private async void ExportDiscovery_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickSavePathAsync("Экспортировать social-bridge bundle", "bridges.ttbridge");
        if (path is not null) await viewModel.ExportDiscoveryBundleAsync(path);
    }

    private async void ImportDiscovery_OnClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is not LocalFirstMessengerViewModel viewModel) return;
        string? path = await PickOpenPathAsync("Импортировать подписанные Nodes и Relays");
        if (path is not null) await viewModel.ImportDiscoveryBundleAsync(path);
    }

    private async Task<string?> PickSavePathAsync(string title, string suggestedName)
    {
        if (TopLevel.GetTopLevel(this)?.StorageProvider is not { } storage) return null;
        IStorageFile? file = await storage.SaveFilePickerAsync(new FilePickerSaveOptions
        {
            Title = title,
            SuggestedFileName = suggestedName
        });
        return file?.TryGetLocalPath();
    }

    private async Task<string?> PickOpenPathAsync(string title)
    {
        if (TopLevel.GetTopLevel(this)?.StorageProvider is not { } storage) return null;
        IReadOnlyList<IStorageFile> files = await storage.OpenFilePickerAsync(new FilePickerOpenOptions
        {
            Title = title,
            AllowMultiple = false
        });
        return files.FirstOrDefault()?.TryGetLocalPath();
    }
}
