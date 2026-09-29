using System.Text;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.Storage;
using Windows.System;

namespace TuratText.Windows;

/// <summary>
/// Каналы: создание и подписка, страница канала с администраторами и подписчиками,
/// комментарии к постам.
/// </summary>
/// <remarks>
/// Права здесь только прячут недоступные кнопки. Решает ядро, а за ним — устройство каждого
/// получателя, которое проверяет подпись и полномочия автора любого изменения канала.
/// Страница канала собирается в коде: набор разделов зависит от прав текущего пользователя.
/// </remarks>
public sealed partial class MainWindow
{
    /// <summary>Отпечаток нарисованной страницы канала: фоновая синхронизация не должна
    /// пересобирать её и стирать то, что пользователь набирает в полях.</summary>
    private string? _channelPageSignature;

    private string? _commentsSignature;
    private string? _commentReplyId;

    private static Brush ThemeBrush(string key) => (Brush)Application.Current.Resources[key];

    // --- создание и подписка --------------------------------------------------

    private ContentDialog NewDialog(string title, UIElement content, string primary)
    {
        return new ContentDialog
        {
            XamlRoot = Root.XamlRoot,
            RequestedTheme = Root.RequestedTheme,
            Title = title,
            Content = content,
            PrimaryButtonText = primary,
            CloseButtonText = "Отмена",
            DefaultButton = ContentDialogButton.Primary,
        };
    }

    private static TextBlock HintText(string text) => new()
    {
        Text = text,
        TextWrapping = TextWrapping.Wrap,
        FontSize = 13,
        Foreground = ThemeBrush("TgHint"),
    };

    private static TextBlock ErrorText() => new()
    {
        TextWrapping = TextWrapping.Wrap,
        Foreground = ThemeBrush("TgDanger"),
        Visibility = Visibility.Collapsed,
    };

    private async void ShowNewChannelDialog()
    {
        var name = new TextBox { Header = "Название канала", MaxLength = 64 };
        var about = new TextBox
        {
            Header = "Описание (необязательно)",
            MaxLength = 255,
            AcceptsReturn = true,
            TextWrapping = TextWrapping.Wrap,
        };
        TextBlock error = ErrorText();
        var panel = new StackPanel { Spacing = 10, MinWidth = 380 };
        panel.Children.Add(HintText(
            "Канал — лента публикаций для подписчиков. Писать в него могут только администраторы, " +
            "подписчики читают, ставят реакции и комментируют. Подписчики не видят друг друга."));
        panel.Children.Add(name);
        panel.Children.Add(about);
        panel.Children.Add(error);
        ContentDialog dialog = NewDialog("Новый канал", panel, "Создать канал");
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            ContentDialogButtonClickDeferral deferral = args.GetDeferral();
            bool created = await ExecuteAsync(new
            {
                command = "create_channel",
                name = name.Text,
                about = about.Text,
                avatar_base64 = (string?)null,
            }, showErrorDialog: false);
            args.Cancel = !created;
            if (!created)
            {
                error.Text = _snapshot?.StatusMessage ?? "Не удалось создать канал";
                error.Visibility = Visibility.Visible;
            }
            deferral.Complete();
        };
        await TryShowDialogAsync(dialog);
    }

    private async void ShowSubscribeDialog()
    {
        var link = new TextBox { Header = "Ссылка на канал", PlaceholderText = "turat://channel/ttch1-…?via=tt1-…" };
        TextBlock error = ErrorText();
        var panel = new StackPanel { Spacing = 10, MinWidth = 380 };
        panel.Children.Add(HintText(
            "Запрос на подписку уйдёт администратору из ссылки. Канал появится, когда он будет в сети " +
            "и пришлёт состояние канала и последние посты."));
        panel.Children.Add(link);
        panel.Children.Add(error);
        ContentDialog dialog = NewDialog("Подписаться на канал", panel, "Подписаться");
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            ContentDialogButtonClickDeferral deferral = args.GetDeferral();
            bool sent = await ExecuteAsync(new { command = "subscribe_channel", link = link.Text.Trim() },
                showErrorDialog: false);
            args.Cancel = !sent;
            if (!sent)
            {
                error.Text = _snapshot?.StatusMessage ?? "Не удалось подписаться";
                error.Visibility = Visibility.Visible;
            }
            deferral.Complete();
        };
        await TryShowDialogAsync(dialog);
    }

    // --- лента канала ---------------------------------------------------------

    private async void OpenComments_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not MessageModel message) return;
        await ExecuteAsync(new { command = "open_comments", post_event_id = message.EventId });
    }

    private void ReactPost_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not MessageModel message) return;
        var menu = new MenuFlyout();
        foreach (string reaction in ReactionSet)
        {
            string value = reaction;
            menu.Items.Add(MenuItem(value, string.Empty, async () =>
                await ExecuteAsync(new { command = "react", event_ids = new[] { message.EventId }, reaction = value })));
        }
        menu.ShowAt((FrameworkElement)sender);
    }

    /// <summary>Нижняя панель подписчика: звук, а у закрытого канала — удаление из списка.</summary>
    private void UpdateChannelBar(ChatModel chat)
    {
        ChannelViewModel? channel = _snapshot?.Channel is { } value && value.ChannelId == chat.UserId ? value : null;
        string? closed = channel switch
        {
            { Closed: true } => "Канал удалён владельцем",
            { Removed: true } => "Администратор удалил вас из канала",
            { AwaitingState: true } => "Запрос на подписку отправлен — ждём администратора",
            { Left: true } => "Вы отписались от канала",
            _ => chat.GroupLeft ? "Вы отписались от канала" : null,
        };
        if (closed is not null)
        {
            ChannelBarText.Text = closed;
            ChannelBarButton.Content = channel?.AwaitingState == true ? "Отменить" : "Удалить канал";
            ChannelBarButton.Foreground = ThemeBrush("TgDanger");
        }
        else
        {
            ChannelBarText.Text = chat.Presence;
            ChannelBarButton.Content = chat.Muted ? "Включить звук" : "Выключить звук";
            ChannelBarButton.Foreground = ThemeBrush("TgAccent");
        }
    }

    private async void ChannelBar_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedChat is not ChatModel { IsChannel: true } chat) return;
        ChannelViewModel? channel = _snapshot.Channel;
        bool active = channel?.Active ?? !chat.GroupLeft;
        if (active)
        {
            await ExecuteAsync(new { command = "set_chat_muted", user_id = chat.UserId, muted = !chat.Muted });
            return;
        }
        if (await ConfirmDeleteChatAsync(chat))
        {
            await ExecuteAsync(new { command = "delete_contact", user_id = chat.UserId });
        }
    }

    // --- страница канала ------------------------------------------------------

    private void ShowChannelPage()
    {
        _channelPageSignature = null;
        FillChannelPage();
        if (_snapshot?.Channel is not null) ChannelPage.Visibility = Visibility.Visible;
    }

    private void CloseChannelPage_Click(object sender, RoutedEventArgs e) => ChannelPage.Visibility = Visibility.Collapsed;

    private static string ChannelSignature(ChannelViewModel channel)
    {
        var builder = new StringBuilder();
        builder.Append(channel.ChannelId).Append('|').Append(channel.Epoch).Append('|')
            .Append(channel.StateLabel).Append('|').Append(channel.MyRole).Append('|')
            .Append(channel.DiscussionJoined).Append('|').Append(channel.InviteLink).AppendLine();
        foreach (ChannelAdminModel admin in channel.Admins)
        {
            builder.Append(admin.UserId).Append(admin.DisplayName).Append(admin.Rights.Summary)
                .Append(admin.Title).Append(admin.CanEdit).AppendLine();
        }
        foreach (ChannelSubscriberModel subscriber in channel.Subscribers)
        {
            builder.Append(subscriber.UserId).Append(subscriber.DisplayName).Append(subscriber.Banned).AppendLine();
        }
        return builder.ToString();
    }

    private void FillChannelPage()
    {
        if (_snapshot?.Channel is not ChannelViewModel channel)
        {
            ChannelPage.Visibility = Visibility.Collapsed;
            return;
        }
        string signature = ChannelSignature(channel);
        if (signature == _channelPageSignature) return;
        _channelPageSignature = signature;

        UIElementCollection content = ChannelPageContent.Children;
        content.Clear();

        // Шапка: фото, название, число подписчиков, описание.
        var avatar = AvatarElement(channel.Name, channel.ChannelId, channel.AvatarBase64, 96);
        if (channel.CanEditInfo)
        {
            var button = new Button
            {
                Style = (Style)Application.Current.Resources["TgIconButton"],
                Padding = new Thickness(0),
                HorizontalAlignment = HorizontalAlignment.Center,
                Content = avatar,
            };
            ToolTipService.SetToolTip(button, "Сменить фото канала");
            button.Click += async (_, _) => await PickChannelAvatarAsync();
            content.Add(button);
        }
        else
        {
            avatar.HorizontalAlignment = HorizontalAlignment.Center;
            content.Add(avatar);
        }
        content.Add(new TextBlock
        {
            Text = channel.Name,
            FontSize = 20,
            FontWeight = FontWeights.SemiBold,
            HorizontalAlignment = HorizontalAlignment.Center,
            TextAlignment = TextAlignment.Center,
            TextWrapping = TextWrapping.Wrap,
            Foreground = ThemeBrush("TgText"),
        });
        content.Add(new TextBlock
        {
            Text = channel.StateLabel + (channel.MyRole switch
            {
                "owner" => " · вы владелец",
                "admin" => " · вы администратор",
                _ => string.Empty,
            }),
            FontSize = 13,
            HorizontalAlignment = HorizontalAlignment.Center,
            Foreground = ThemeBrush("TgHint"),
        });
        if (channel.About.Length > 0)
        {
            content.Add(new TextBlock
            {
                Text = channel.About,
                FontSize = 13,
                TextWrapping = TextWrapping.Wrap,
                TextAlignment = TextAlignment.Center,
                HorizontalAlignment = HorizontalAlignment.Center,
                Foreground = ThemeBrush("TgText"),
            });
        }

        if (channel.InviteLink is string link)
        {
            content.Add(SectionTitle("Ссылка-приглашение"));
            content.Add(new TextBlock
            {
                Text = link,
                FontFamily = new FontFamily("Consolas"),
                FontSize = 11,
                TextWrapping = TextWrapping.Wrap,
                IsTextSelectionEnabled = true,
                Foreground = ThemeBrush("TgText"),
            });
            content.Add(HintText("По этой ссылке подписка идёт через вас: запрос обработает ваше устройство."));
            content.Add(ButtonRow(
                ("Копировать ссылку", () => { CopyText(link); StatusText.Text = "Ссылка скопирована"; }, false),
                ("Пригласить контакты", ShowInviteToChannelDialog, false)));
        }

        if (channel.CanEditInfo)
        {
            content.Add(SectionTitle("Данные канала"));
            var name = new TextBox { Header = "Название", MaxLength = 64, Text = channel.Name };
            var about = new TextBox
            {
                Header = "Описание",
                MaxLength = 255,
                AcceptsReturn = true,
                TextWrapping = TextWrapping.Wrap,
                Text = channel.About,
            };
            content.Add(name);
            content.Add(about);
            var buttons = new List<(string, Action, bool)>
            {
                ("Сохранить", () => _ = SaveChannelInfoAsync(channel, name.Text.Trim(), about.Text.Trim()), true),
            };
            if (channel.AvatarBase64 is not null)
            {
                buttons.Add(("Убрать фото", () => _ = ExecuteAsync(new
                {
                    command = "update_channel_info", channel_id = channel.ChannelId, name = channel.Name,
                    about = channel.About, avatar_base64 = (string?)null,
                }), false));
            }
            content.Add(ButtonRow([.. buttons]));

            content.Add(SectionTitle("Настройки"));
            content.Add(SettingToggle("Подписывать посты", "Имя автора под постом", "Посты от имени канала",
                channel.Settings.SignPosts, value => _ = ExecuteAsync(new
                {
                    command = "set_channel_settings", channel_id = channel.ChannelId, sign_posts = value,
                    comments_enabled = channel.Settings.CommentsEnabled,
                })));
            content.Add(SettingToggle("Комментарии", "Подписчики могут комментировать", "Комментарии выключены",
                channel.Settings.CommentsEnabled, value => _ = ExecuteAsync(new
                {
                    command = "set_channel_settings", channel_id = channel.ChannelId,
                    sign_posts = channel.Settings.SignPosts, comments_enabled = value,
                })));
        }

        content.Add(SectionTitle("Обсуждение"));
        if (channel.Settings.DiscussionGroupId is string discussion)
        {
            string title = channel.Settings.DiscussionGroupName.Length > 0
                ? channel.Settings.DiscussionGroupName
                : "Группа обсуждения";
            content.Add(new TextBlock
            {
                Text = title,
                FontSize = 14,
                FontWeight = FontWeights.SemiBold,
                Foreground = ThemeBrush("TgText"),
            });
            content.Add(HintText(channel.DiscussionJoined
                ? "Все новые посты пересылаются в эту группу."
                : "Вы не участник этой группы: попросите администратора добавить вас."));
            if (channel.DiscussionJoined)
            {
                content.Add(ButtonRow(("Открыть обсуждение", () =>
                {
                    ChannelPage.Visibility = Visibility.Collapsed;
                    _ = OpenChatAsync(discussion);
                }, false)));
            }
        }
        else
        {
            content.Add(HintText("Группа обсуждения не привязана. Комментарии под постами работают и без неё."));
        }
        if (channel.CanEditInfo)
        {
            content.Add(ButtonRow((channel.Settings.DiscussionGroupId is null ? "Привязать группу" : "Сменить группу",
                () => ShowDiscussionDialog(channel), false)));
        }

        content.Add(SectionTitle(channel.MyRole is null ? "Администраторы" : $"Администраторы · {channel.Admins.Count}"));
        if (channel.CanAddAdmins)
        {
            content.Add(ButtonRow(("Добавить администратора", () => ShowPickAdminDialog(channel), false)));
        }
        foreach (ChannelAdminModel admin in channel.Admins)
        {
            string subtitle = admin.Role == "owner"
                ? "владелец"
                : (admin.Title.Length > 0 ? admin.Title + " · " : string.Empty) + admin.Rights.Summary;
            content.Add(PersonRow(admin.DisplayName + (admin.IsSelf ? " (вы)" : string.Empty), subtitle,
                admin.Role == "owner" ? "владелец" : "админ", admin.UserId, admin.AvatarBase64,
                AdminActions(channel, admin)));
        }

        if (channel.MyRole is not null)
        {
            content.Add(SectionTitle(Formatting.Subscribers(channel.SubscriberCount)));
            if (channel.Subscribers.Count == 0)
            {
                content.Add(HintText("Пока никого. Поделитесь ссылкой-приглашением или пригласите контакты."));
            }
            foreach (ChannelSubscriberModel subscriber in channel.Subscribers)
            {
                string subtitle = subscriber.Banned
                    ? "заблокирован(а)"
                    : "подписан(а) с " + Formatting.DateSeparator(subscriber.SubscribedAtUnixMilliseconds);
                content.Add(PersonRow(subscriber.DisplayName, subtitle, null, subscriber.UserId,
                    subscriber.AvatarBase64, SubscriberActions(channel, subscriber)));
            }
        }

        content.Add(SectionTitle("Безопасность"));
        content.Add(HintText(
            "Каждый пост шифруется отдельно для каждого подписчика его личным сквозным каналом. Права " +
            "администраторов проверяет устройство каждого получателя. Комментарии подписчиков разносит " +
            "администратор, но подделать их не может: подпись автора проверяется у всех."));
        content.Add(new TextBlock
        {
            Text = "ChannelID: " + channel.ChannelId,
            FontFamily = new FontFamily("Consolas"),
            FontSize = 11,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            Foreground = ThemeBrush("TgHint"),
        });

        var danger = new List<(string, Action, bool)>();
        if (channel.Active && channel.MyRole != "owner")
        {
            danger.Add((channel.MyRole is null ? "Отписаться" : "Сложить полномочия и отписаться", () => _ = LeaveChannelAsync(channel), false));
        }
        if (channel.Active && channel.MyRole == "owner")
        {
            danger.Add(("Удалить канал у всех", () => _ = CloseChannelAsync(channel), false));
        }
        else
        {
            danger.Add(("Удалить из списка", () => DeleteChat_Click(this, new RoutedEventArgs()), false));
        }
        StackPanel dangerRow = ButtonRow([.. danger]);
        foreach (Button button in dangerRow.Children.OfType<Button>()) button.Foreground = ThemeBrush("TgDanger");
        dangerRow.Margin = new Thickness(0, 10, 0, 0);
        content.Add(dangerRow);

        DispatcherQueue.TryEnqueue(() => ApplyFontToTree(ChannelPage, _font.Family));
    }

    private static TextBlock SectionTitle(string text) => new()
    {
        Text = text,
        Style = (Style)Application.Current.Resources["TgSectionTitle"],
    };

    private static StackPanel ButtonRow(params (string Title, Action Run, bool Accent)[] buttons)
    {
        var row = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 8, Margin = new Thickness(0, 4, 0, 0) };
        foreach ((string title, Action run, bool accent) in buttons)
        {
            var button = new Button { Content = title };
            if (accent)
            {
                button.Background = ThemeBrush("TgAccent");
                button.Foreground = ThemeBrush("TgOnAccent");
                button.BorderThickness = new Thickness(0);
            }
            button.Click += (_, _) => run();
            row.Children.Add(button);
        }
        return row;
    }

    private ToggleSwitch SettingToggle(string header, string on, string off, bool value, Action<bool> changed)
    {
        var toggle = new ToggleSwitch { Header = header, OnContent = on, OffContent = off, IsOn = value };
        toggle.Toggled += (_, _) =>
        {
            if (!_updating) changed(toggle.IsOn);
        };
        return toggle;
    }

    private static Grid AvatarElement(string name, string key, string? avatarBase64, double size)
    {
        var grid = new Grid { Width = size, Height = size };
        var circle = new Border { CornerRadius = new CornerRadius(size / 2), Background = AvatarPalette.For(key) };
        var inner = new Grid();
        inner.Children.Add(new TextBlock
        {
            Text = Formatting.Initials(name),
            FontSize = size * 0.33,
            FontWeight = FontWeights.SemiBold,
            Foreground = new SolidColorBrush(Microsoft.UI.Colors.White),
            HorizontalAlignment = HorizontalAlignment.Center,
            VerticalAlignment = VerticalAlignment.Center,
        });
        inner.Children.Add(new Image { Source = Images.Decode(avatarBase64), Stretch = Stretch.UniformToFill });
        circle.Child = inner;
        grid.Children.Add(circle);
        return grid;
    }

    /// <summary>Строка человека: аватар, имя, пояснение, метка роли и меню действий.</summary>
    private Grid PersonRow(
        string name,
        string subtitle,
        string? badge,
        string key,
        string? avatarBase64,
        List<(string Title, Func<Task> Run)> actions)
    {
        var row = new Grid { Padding = new Thickness(6, 7, 6, 7), ColumnSpacing = 12, CornerRadius = new CornerRadius(12) };
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
        row.Children.Add(AvatarElement(name, key, avatarBase64, 42));

        var text = new StackPanel { VerticalAlignment = VerticalAlignment.Center };
        text.Children.Add(new TextBlock
        {
            Text = name,
            FontSize = 14,
            FontWeight = FontWeights.SemiBold,
            TextTrimming = TextTrimming.CharacterEllipsis,
            Foreground = ThemeBrush("TgText"),
        });
        text.Children.Add(new TextBlock
        {
            Text = subtitle,
            FontSize = 12,
            TextTrimming = TextTrimming.CharacterEllipsis,
            Foreground = ThemeBrush("TgHint"),
        });
        Grid.SetColumn(text, 1);
        row.Children.Add(text);

        if (badge is not null)
        {
            var pill = new Border
            {
                CornerRadius = new CornerRadius(10),
                Padding = new Thickness(8, 2, 8, 2),
                VerticalAlignment = VerticalAlignment.Center,
                Background = ThemeBrush("TgAccentSoft"),
                Child = new TextBlock
                {
                    Text = badge,
                    FontSize = 11,
                    FontWeight = FontWeights.SemiBold,
                    Foreground = ThemeBrush("TgText"),
                },
            };
            Grid.SetColumn(pill, 2);
            row.Children.Add(pill);
        }

        if (actions.Count > 0)
        {
            MenuFlyout Menu()
            {
                var menu = new MenuFlyout();
                foreach ((string title, Func<Task> run) in actions)
                {
                    menu.Items.Add(MenuItem(title, string.Empty, () => _ = run()));
                }
                return menu;
            }
            var more = new Button
            {
                Style = (Style)Application.Current.Resources["TgIconButton"],
                Content = new SymbolIcon(Symbol.More),
                Flyout = Menu(),
            };
            ToolTipService.SetToolTip(more, "Действия");
            Grid.SetColumn(more, 3);
            row.Children.Add(more);
            row.ContextFlyout = Menu();
        }
        return row;
    }

    private List<(string Title, Func<Task> Run)> AdminActions(ChannelViewModel channel, ChannelAdminModel admin)
    {
        var actions = new List<(string Title, Func<Task> Run)>();
        if (admin.IsSelf) return actions;
        if (admin.CanEdit)
        {
            actions.Add(("Изменить права", () =>
            {
                ShowAdminRightsDialog(channel, admin.UserId, admin.DisplayName);
                return Task.CompletedTask;
            }));
        }
        if (channel.MyRole == "owner" && admin.Role == "admin")
        {
            actions.Add(("Передать канал", async () =>
            {
                if (await ConfirmAsync("Передать канал?",
                        $"{admin.DisplayName} станет владельцем канала, а вы — администратором со всеми правами."))
                {
                    await ExecuteAsync(new { command = "transfer_channel_ownership", channel_id = channel.ChannelId, user_id = admin.UserId });
                }
            }));
        }
        if (admin.CanEdit)
        {
            actions.Add(("Разжаловать", async () =>
            {
                if (await ConfirmAsync("Разжаловать администратора?",
                        $"{admin.DisplayName} останется подписчиком канала, но больше не сможет публиковать."))
                {
                    await ExecuteAsync(new { command = "remove_channel_admin", channel_id = channel.ChannelId, user_id = admin.UserId });
                }
            }));
        }
        return actions;
    }

    private List<(string Title, Func<Task> Run)> SubscriberActions(ChannelViewModel channel, ChannelSubscriberModel subscriber)
    {
        var actions = new List<(string Title, Func<Task> Run)>();
        if (channel.CanAddAdmins && !subscriber.Banned)
        {
            actions.Add(("Назначить администратором", () =>
            {
                ShowAdminRightsDialog(channel, subscriber.UserId, subscriber.DisplayName);
                return Task.CompletedTask;
            }));
        }
        if (channel.CanBan && !subscriber.Banned)
        {
            actions.Add(("Удалить из канала", async () =>
            {
                if (await ConfirmAsync("Удалить подписчика?",
                        $"{subscriber.DisplayName} перестанет получать посты, но сможет подписаться по ссылке."))
                {
                    await ExecuteAsync(new { command = "remove_channel_subscriber", channel_id = channel.ChannelId, user_id = subscriber.UserId, ban = false });
                }
            }));
            actions.Add(("Заблокировать", async () =>
            {
                if (await ConfirmAsync("Заблокировать подписчика?",
                        $"{subscriber.DisplayName} перестанет получать посты и не сможет подписаться снова."))
                {
                    await ExecuteAsync(new { command = "remove_channel_subscriber", channel_id = channel.ChannelId, user_id = subscriber.UserId, ban = true });
                }
            }));
        }
        if (channel.CanBan && subscriber.Banned)
        {
            actions.Add(("Разблокировать", () =>
                ExecuteAsync(new { command = "unban_channel_subscriber", channel_id = channel.ChannelId, user_id = subscriber.UserId })));
        }
        return actions;
    }

    private async Task SaveChannelInfoAsync(ChannelViewModel channel, string name, string about)
    {
        if (name == channel.Name && about == channel.About) return;
        _channelPageSignature = null;
        await ExecuteAsync(new
        {
            command = "update_channel_info", channel_id = channel.ChannelId, name, about,
            avatar_base64 = channel.AvatarBase64,
        });
    }

    private async Task PickChannelAvatarAsync()
    {
        StorageFile? file = await OpenFileAsync([".png", ".jpg", ".jpeg", ".bmp"]);
        if (file is null) return;
        // Фото едет внутри состояния канала: те же пределы, что и у группы.
        string? encoded = await EncodeAvatarAsync(file, 160, 0.82);
        if (encoded is { Length: > GroupAvatarLimit }) encoded = await EncodeAvatarAsync(file, 112, 0.6);
        if (encoded is null)
        {
            await ShowErrorAsync("Не удалось прочитать изображение");
            return;
        }
        if (encoded.Length > GroupAvatarLimit)
        {
            await ShowErrorAsync("Фото слишком детальное для канала — выберите другое");
            return;
        }
        if (_snapshot?.Channel is not ChannelViewModel channel) return;
        _channelPageSignature = null;
        await ExecuteAsync(new
        {
            command = "update_channel_info", channel_id = channel.ChannelId, name = channel.Name,
            about = channel.About, avatar_base64 = encoded,
        });
    }

    private async Task LeaveChannelAsync(ChannelViewModel channel)
    {
        if (await ConfirmAsync("Отписаться от канала?",
                "История останется на этом устройстве, но новые посты приходить не будут."))
        {
            await ExecuteAsync(new { command = "leave_channel", channel_id = channel.ChannelId });
        }
    }

    private async Task CloseChannelAsync(ChannelViewModel channel)
    {
        if (await ConfirmAsync("Удалить канал?",
                "Канал закроется у всех подписчиков: публиковать в него больше будет нельзя. Это необратимо."))
        {
            await ExecuteAsync(new { command = "close_channel", channel_id = channel.ChannelId });
        }
    }

    /// <summary>Права администратора: семь переключателей и подпись, как в Telegram.</summary>
    private async void ShowAdminRightsDialog(ChannelViewModel channel, string userId, string displayName)
    {
        ChannelAdminModel? existing = channel.Admins.FirstOrDefault(value => value.UserId == userId);
        ChannelRightsModel current = existing?.Rights ?? new ChannelRightsModel(true, true, true);
        ChannelRightsModel mine = channel.MyRights;
        bool owner = channel.MyRole == "owner";
        CheckBox Right(string text, bool value, bool allowed) =>
            new() { Content = text, IsChecked = value, IsEnabled = owner || allowed };

        CheckBox post = Right("Публиковать посты", current.PostMessages, mine.PostMessages);
        CheckBox edit = Right("Редактировать чужие посты", current.EditMessages, mine.EditMessages);
        CheckBox delete = Right("Удалять чужие посты и комментарии", current.DeleteMessages, mine.DeleteMessages);
        CheckBox invite = Right("Приглашать подписчиков", current.InviteUsers, mine.InviteUsers);
        CheckBox info = Right("Менять данные и настройки канала", current.ChangeInfo, mine.ChangeInfo);
        CheckBox ban = Right("Удалять и блокировать подписчиков", current.BanUsers, mine.BanUsers);
        CheckBox admins = Right("Назначать администраторов", current.AddAdmins, mine.AddAdmins);
        var title = new TextBox { Header = "Подпись", PlaceholderText = "Например, «Редактор»", MaxLength = 16, Text = existing?.Title ?? string.Empty };
        TextBlock error = ErrorText();

        var panel = new StackPanel { Spacing = 4, MinWidth = 380 };
        panel.Children.Add(HintText(displayName + " · " + Formatting.ShortId(userId)));
        foreach (CheckBox box in new[] { post, edit, delete, invite, info, ban, admins }) panel.Children.Add(box);
        if (!owner) panel.Children.Add(HintText("Выдать можно только те права, которые есть у вас самих."));
        panel.Children.Add(title);
        panel.Children.Add(error);

        ContentDialog dialog = NewDialog(existing is null ? "Новый администратор" : "Права администратора", panel,
            existing is null ? "Назначить" : "Сохранить");
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            ContentDialogButtonClickDeferral deferral = args.GetDeferral();
            bool saved = await ExecuteAsync(new
            {
                command = "set_channel_admin",
                channel_id = channel.ChannelId,
                user_id = userId,
                rights = new
                {
                    postMessages = post.IsChecked == true,
                    editMessages = edit.IsChecked == true,
                    deleteMessages = delete.IsChecked == true,
                    inviteUsers = invite.IsChecked == true,
                    changeInfo = info.IsChecked == true,
                    banUsers = ban.IsChecked == true,
                    addAdmins = admins.IsChecked == true,
                },
                title = title.Text.Trim(),
            }, showErrorDialog: false);
            args.Cancel = !saved;
            if (!saved)
            {
                error.Text = _snapshot?.StatusMessage ?? "Не удалось сохранить права";
                error.Visibility = Visibility.Visible;
            }
            deferral.Complete();
        };
        await TryShowDialogAsync(dialog);
    }

    /// <summary>Кого назначить администратором: подписчики канала и принятые контакты.</summary>
    private async void ShowPickAdminDialog(ChannelViewModel channel)
    {
        HashSet<string> present = [.. channel.Admins.Select(admin => admin.UserId)];
        var candidates = new List<(string UserId, string Name)>();
        foreach (ChannelSubscriberModel subscriber in channel.Subscribers.Where(value => !value.Banned && !present.Contains(value.UserId)))
        {
            candidates.Add((subscriber.UserId, subscriber.DisplayName + " · подписчик"));
        }
        foreach (ChatModel chat in AcceptedContacts().Where(chat => !chat.IsChannel && !present.Contains(chat.UserId)))
        {
            if (candidates.All(value => value.UserId != chat.UserId)) candidates.Add((chat.UserId, chat.DisplayName + " · контакт"));
        }
        if (candidates.Count == 0)
        {
            await ShowErrorAsync("Назначить можно подписчика канала или принятый контакт.");
            return;
        }
        var list = new ListView
        {
            SelectionMode = ListViewSelectionMode.Single,
            Height = 320,
            ItemsSource = candidates.Select(value => value.Name).ToList(),
        };
        ContentDialog dialog = NewDialog("Добавить администратора", list, "Далее");
        ContentDialogResult result;
        try
        {
            result = await dialog.ShowAsync();
        }
        catch (Exception exception)
        {
            StatusText.Text = exception.Message;
            return;
        }
        if (result != ContentDialogResult.Primary || list.SelectedIndex < 0) return;
        (string userId, string name) = candidates[list.SelectedIndex];
        ShowAdminRightsDialog(channel, userId, name[..name.LastIndexOf(" · ", StringComparison.Ordinal)]);
    }

    private async void ShowInviteToChannelDialog()
    {
        if (_snapshot?.Channel is not ChannelViewModel channel) return;
        HashSet<string> present = [.. channel.Subscribers.Select(value => value.UserId), .. channel.Admins.Select(value => value.UserId)];
        List<ChatModel> candidates = [.. AcceptedContacts().Where(chat => !chat.IsChannel && !present.Contains(chat.UserId))];
        if (candidates.Count == 0)
        {
            await ShowErrorAsync("Все ваши контакты уже подписаны.");
            return;
        }
        var list = new ListView
        {
            SelectionMode = ListViewSelectionMode.Multiple,
            Height = 320,
            ItemsSource = candidates,
            ItemTemplate = (DataTemplate)Root.Resources["ContactPickTemplate"],
        };
        TextBlock error = ErrorText();
        var panel = new StackPanel { Spacing = 8, MinWidth = 380 };
        panel.Children.Add(HintText("Каждый получит приглашение и сам решит, подписываться ли."));
        panel.Children.Add(list);
        panel.Children.Add(error);
        ContentDialog dialog = NewDialog("Пригласить в канал", panel, "Пригласить");
        dialog.PrimaryButtonClick += async (_, args) =>
        {
            string[] ids = [.. list.SelectedItems.OfType<ChatModel>().Select(chat => chat.UserId)];
            if (ids.Length == 0)
            {
                args.Cancel = true;
                error.Text = "Отметьте, кого пригласить";
                error.Visibility = Visibility.Visible;
                return;
            }
            ContentDialogButtonClickDeferral deferral = args.GetDeferral();
            bool sent = await ExecuteAsync(new { command = "invite_to_channel", channel_id = channel.ChannelId, user_ids = ids },
                showErrorDialog: false);
            args.Cancel = !sent;
            if (!sent)
            {
                error.Text = _snapshot?.StatusMessage ?? "Не удалось отправить приглашения";
                error.Visibility = Visibility.Visible;
            }
            deferral.Complete();
        };
        await TryShowDialogAsync(dialog);
    }

    /// <summary>Группа обсуждения: только та, в которой пользователь сам состоит.</summary>
    private async void ShowDiscussionDialog(ChannelViewModel channel)
    {
        List<ChatModel> groups = [.. (_snapshot?.Chats ?? []).Where(chat => chat.IsGroup && chat.CanWrite)];
        var list = new ListView
        {
            SelectionMode = ListViewSelectionMode.Single,
            Height = 300,
            ItemsSource = groups,
            ItemTemplate = (DataTemplate)Root.Resources["ContactPickTemplate"],
        };
        var panel = new StackPanel { Spacing = 8, MinWidth = 380 };
        panel.Children.Add(HintText(groups.Count == 0
            ? "Сначала создайте группу — привязать можно только группу, в которой вы состоите."
            : "Все новые посты будут пересылаться в выбранную группу."));
        panel.Children.Add(list);
        ContentDialog dialog = NewDialog("Группа обсуждения", panel, "Привязать");
        if (channel.Settings.DiscussionGroupId is not null) dialog.SecondaryButtonText = "Отвязать";
        ContentDialogResult result;
        try
        {
            result = await dialog.ShowAsync();
        }
        catch (Exception exception)
        {
            StatusText.Text = exception.Message;
            return;
        }
        if (result == ContentDialogResult.Secondary)
        {
            await ExecuteAsync(new { command = "link_discussion_group", channel_id = channel.ChannelId, group_id = (string?)null });
        }
        else if (result == ContentDialogResult.Primary && list.SelectedItem is ChatModel group)
        {
            await ExecuteAsync(new { command = "link_discussion_group", channel_id = channel.ChannelId, group_id = group.UserId });
        }
    }

    // --- комментарии ------------------------------------------------------------

    /// <summary>Ветка комментариев живёт в снимке ядра: страница открыта, пока она выбрана.</summary>
    private void UpdateCommentsPage()
    {
        AppSnapshot? snapshot = _snapshot;
        ChannelViewModel? channel = snapshot?.Channel;
        string? postId = channel?.ThreadPostEventId;
        if (snapshot is null || channel is null || postId is null || snapshot.SelectedContactId != channel.ChannelId)
        {
            if (CommentsPage.Visibility == Visibility.Visible)
            {
                CommentsPage.Visibility = Visibility.Collapsed;
                _commentsSignature = null;
                _commentReplyId = null;
                CommentInput.Text = string.Empty;
            }
            return;
        }
        bool opening = CommentsPage.Visibility != Visibility.Visible;
        CommentsPage.Visibility = Visibility.Visible;
        IReadOnlyList<MessageModel> comments = snapshot.Comments ?? [];
        CommentsTitle.Text = comments.Count == 0 ? "Комментарии" : Formatting.Comments(comments.Count);
        CommentsSubtitle.Text = channel.Name;
        CommentsComposer.Visibility = channel.CanComment ? Visibility.Visible : Visibility.Collapsed;
        CommentsDisabled.Visibility = channel.CanComment ? Visibility.Collapsed : Visibility.Visible;
        CommentsDisabled.Text = channel.Active ? "Комментарии в этом канале выключены" : "Комментировать могут только подписчики";

        var signature = new StringBuilder(postId).AppendLine();
        foreach (MessageModel comment in comments)
        {
            signature.Append(comment.EventId).Append(comment.Text).Append(comment.Delivered).Append(comment.SenderName).AppendLine();
        }
        if (!opening && signature.ToString() == _commentsSignature) return;
        _commentsSignature = signature.ToString();

        UIElementCollection content = CommentsContent.Children;
        content.Clear();
        MessageModel? post = snapshot.Messages.FirstOrDefault(message => message.EventId == postId);
        var card = new Border
        {
            CornerRadius = new CornerRadius(14),
            Padding = new Thickness(12, 9, 12, 9),
            Margin = new Thickness(0, 0, 0, 8),
            Background = ThemeBrush("TgBubbleIn"),
            BorderThickness = new Thickness(1),
            BorderBrush = ThemeBrush("TgGlassRim"),
        };
        var cardText = new StackPanel { Spacing = 2 };
        cardText.Children.Add(new TextBlock { Text = channel.Name, FontSize = 13, FontWeight = FontWeights.SemiBold, Foreground = ThemeBrush("TgAccent") });
        cardText.Children.Add(new TextBlock
        {
            Text = post?.Quote ?? "Пост",
            FontSize = 14,
            TextWrapping = TextWrapping.Wrap,
            MaxLines = 8,
            TextTrimming = TextTrimming.CharacterEllipsis,
            Foreground = ThemeBrush("TgBubbleInText"),
        });
        card.Child = cardText;
        content.Add(card);

        if (comments.Count == 0)
        {
            content.Add(new TextBlock
            {
                Text = channel.CanComment ? "Будьте первым, кто оставит комментарий" : "Комментариев нет",
                HorizontalAlignment = HorizontalAlignment.Center,
                Margin = new Thickness(0, 16, 0, 0),
                Foreground = ThemeBrush("TgHint"),
                FontSize = 13,
            });
        }
        Dictionary<string, MessageModel> byId = comments.ToDictionary(value => value.EventId);
        foreach (MessageModel comment in comments)
        {
            content.Add(CommentBubble(channel, comment,
                comment.ReplyToEventId is string replyId && byId.TryGetValue(replyId, out MessageModel? replied) ? replied : null));
        }
        DispatcherQueue.TryEnqueue(() =>
        {
            ApplyFontToTree(CommentsPage, _font.Family);
            CommentsScroll.UpdateLayout();
            CommentsScroll.ChangeView(null, CommentsScroll.ScrollableHeight, null, disableAnimation: true);
        });
    }

    private Border CommentBubble(ChannelViewModel channel, MessageModel comment, MessageModel? replied)
    {
        bool outgoing = comment.Outgoing;
        var body = new StackPanel { Spacing = 2 };
        if (!outgoing)
        {
            body.Children.Add(new TextBlock
            {
                Text = comment.SenderName ?? Formatting.ShortId(comment.SenderUserId),
                FontSize = 13,
                FontWeight = FontWeights.SemiBold,
                TextTrimming = TextTrimming.CharacterEllipsis,
                Foreground = AvatarPalette.For(comment.SenderUserId),
            });
        }
        if (replied is not null)
        {
            body.Children.Add(new TextBlock
            {
                Text = "↩ " + (replied.Outgoing ? "Вы" : replied.SenderName ?? string.Empty) + ": " + replied.Text,
                FontSize = 12,
                MaxLines = 1,
                TextTrimming = TextTrimming.CharacterEllipsis,
                Foreground = ThemeBrush(outgoing ? "TgMetaOut" : "TgMetaIn"),
            });
        }
        body.Children.Add(new TextBlock
        {
            Text = comment.Text,
            FontSize = 14,
            TextWrapping = TextWrapping.Wrap,
            IsTextSelectionEnabled = true,
            Foreground = ThemeBrush(outgoing ? "TgBubbleOutText" : "TgBubbleInText"),
        });
        body.Children.Add(new TextBlock
        {
            Text = comment.TimeLabel + (outgoing && !comment.Delivered ? " · отправляется" : string.Empty),
            FontSize = 11,
            HorizontalAlignment = HorizontalAlignment.Right,
            Foreground = ThemeBrush(outgoing ? "TgMetaOut" : "TgMetaIn"),
        });

        var bubble = new Border
        {
            CornerRadius = new CornerRadius(14),
            Padding = new Thickness(12, 7, 12, 7),
            MaxWidth = 520,
            HorizontalAlignment = outgoing ? HorizontalAlignment.Right : HorizontalAlignment.Left,
            Background = ThemeBrush(outgoing ? "TgBubbleOut" : "TgBubbleIn"),
            BorderThickness = new Thickness(1),
            BorderBrush = ThemeBrush("TgGlassRim"),
            Child = body,
        };

        var menu = new MenuFlyout();
        if (channel.CanComment)
        {
            menu.Items.Add(MenuItem("Ответить", string.Empty, () =>
            {
                _commentReplyId = comment.EventId;
                CommentReplyText.Text = "↩ " + (comment.Outgoing ? "Вы" : comment.SenderName ?? string.Empty) + ": " + comment.Text;
                CommentReplyBanner.Visibility = Visibility.Visible;
                CommentInput.Focus(FocusState.Programmatic);
            }));
        }
        menu.Items.Add(MenuItem("Копировать", string.Empty, () => CopyText(comment.Text)));
        if (comment.Outgoing || channel.CanDeleteMessages)
        {
            menu.Items.Add(MenuItem("Удалить", string.Empty, async () =>
            {
                if (comment.Outgoing || await ConfirmAsync("Удалить комментарий?", "Комментарий исчезнет у всех подписчиков."))
                    await ExecuteAsync(new { command = "delete_messages", event_ids = new[] { comment.EventId } });
            }));
        }
        bubble.ContextFlyout = menu;
        return bubble;
    }

    private async void CloseComments_Click(object sender, RoutedEventArgs e) =>
        await ExecuteAsync(new { command = "open_comments", post_event_id = (string?)null });

    private void CancelCommentReply_Click(object sender, RoutedEventArgs e)
    {
        _commentReplyId = null;
        CommentReplyBanner.Visibility = Visibility.Collapsed;
    }

    private async void SendComment_Click(object sender, RoutedEventArgs e) => await SendCommentAsync();

    private async void CommentInput_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key != VirtualKey.Enter) return;
        e.Handled = true;
        await SendCommentAsync();
    }

    private async Task SendCommentAsync()
    {
        string text = CommentInput.Text.Trim();
        if (text.Length == 0 || _snapshot?.Channel?.ThreadPostEventId is not string postId) return;
        bool sent = await ExecuteAsync(new
        {
            command = "send_comment",
            post_event_id = postId,
            text,
            reply_to_event_id = _commentReplyId,
        });
        if (!sent) return;
        CommentInput.Text = string.Empty;
        _commentReplyId = null;
        CommentReplyBanner.Visibility = Visibility.Collapsed;
    }
}
