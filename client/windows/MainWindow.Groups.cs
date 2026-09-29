using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Windows.Storage;

namespace TuratText.Windows;

/// <summary>
/// Групповые чаты: создание группы, её страница, участники и роли.
/// </summary>
/// <remarks>
/// Права здесь только прячут недоступные кнопки. Решает ядро, а за ним — устройство каждого
/// участника, которое проверяет подпись и полномочия автора любого изменения группы.
/// </remarks>
public sealed partial class MainWindow
{
    /// <summary>Совпадает с пределом ядра: фото группы должно пролезать в публичный ящик участника.</summary>
    private const int GroupAvatarLimit = 32_000;

    /// <summary>Группа и номер её состояния, под которые заполнены поля редактирования.</summary>
    /// <remarks>Фоновая синхронизация не должна стирать название, которое пользователь как раз набирает.</remarks>
    private string? _groupFormKey;

    private void NewChat_Click(object sender, RoutedEventArgs e)
    {
        var menu = new MenuFlyout();
        menu.Items.Add(MenuItem("Новый диалог", "", () => NewContact_Click(sender, e)));
        menu.Items.Add(MenuItem("Новая группа", "", ShowNewGroupDialog));
        menu.Items.Add(MenuItem("Новый канал", "\uE789", ShowNewChannelDialog));
        menu.Items.Add(MenuItem("Подписаться на канал", "\uE71B", ShowSubscribeDialog));
        menu.ShowAt((FrameworkElement)sender);
    }

    private IEnumerable<ChatModel> AcceptedContacts() =>
        _snapshot?.Chats.Where(chat => !chat.IsGroup && !chat.IsChannel && !chat.PendingApproval) ?? [];

    private async void ShowNewGroupDialog()
    {
        if (_snapshot is null) return;
        NewGroupName.Text = string.Empty;
        NewGroupError.Visibility = Visibility.Collapsed;
        List<ChatModel> contacts = [.. AcceptedContacts()];
        NewGroupContacts.ItemsSource = contacts;
        NewGroupEmpty.Visibility = contacts.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        await TryShowDialogAsync(NewGroupDialog);
    }

    private async void NewGroupDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        string[] members = [.. NewGroupContacts.SelectedItems.OfType<ChatModel>().Select(chat => chat.UserId)];
        bool created = await ExecuteAsync(new
        {
            command = "create_group",
            name = NewGroupName.Text,
            about = string.Empty,
            avatar_base64 = (string?)null,
            member_ids = members,
        }, showErrorDialog: false);
        args.Cancel = !created;
        if (!created)
        {
            NewGroupError.Text = _snapshot?.StatusMessage ?? "Не удалось создать группу";
            NewGroupError.Visibility = Visibility.Visible;
        }
        deferral.Complete();
    }

    // --- страница группы ------------------------------------------------------

    private void ShowGroupPage()
    {
        _groupFormKey = null;
        FillGroupPage();
        if (_snapshot?.Group is not null) GroupPage.Visibility = Visibility.Visible;
    }

    private void CloseGroupPage_Click(object sender, RoutedEventArgs e) => GroupPage.Visibility = Visibility.Collapsed;

    private void FillGroupPage()
    {
        if (_snapshot?.Group is not GroupViewModel group)
        {
            GroupPage.Visibility = Visibility.Collapsed;
            return;
        }
        bool wasUpdating = _updating;
        _updating = true;
        try
        {
            GroupInitial.Text = Formatting.Initials(group.Name);
            GroupAvatar.Source = Images.Decode(group.AvatarBase64);
            GroupAvatarButton.IsEnabled = group.CanEditInfo;
            GroupAvatarBadge.Visibility = group.CanEditInfo ? Visibility.Visible : Visibility.Collapsed;
            GroupTitle.Text = group.Name;
            GroupSubtitle.Text = group.Left ? "вы не участник группы"
                : group.PendingInvite ? "приглашение ещё не принято"
                : $"{Formatting.Members(group.Members.Count)} · {RoleTitle(group.MyRole)}";
            GroupAboutText.Text = group.About;
            GroupAboutText.Visibility = group.About.Length > 0 ? Visibility.Visible : Visibility.Collapsed;

            GroupEditPanel.Visibility = group.CanEditInfo ? Visibility.Visible : Visibility.Collapsed;
            string formKey = group.GroupId + ":" + group.Epoch;
            if (_groupFormKey != formKey)
            {
                _groupFormKey = formKey;
                GroupNameInput.Text = group.Name;
                GroupAboutInput.Text = group.About;
            }
            RemoveGroupAvatarButton.Visibility = group.AvatarBase64 is null ? Visibility.Collapsed : Visibility.Visible;

            // Права участников меняют администраторы — те же, кто может исключать.
            GroupPermissionsPanel.Visibility = group.CanRemoveMembers ? Visibility.Visible : Visibility.Collapsed;
            MembersCanInviteToggle.IsOn = group.Permissions.MembersCanInvite;
            MembersCanEditToggle.IsOn = group.Permissions.MembersCanEditInfo;

            GroupMembersTitle.Text = Formatting.Members(group.Members.Count);
            AddGroupMembersButton.Visibility = group.CanInvite ? Visibility.Visible : Visibility.Collapsed;
            foreach (GroupMemberModel member in group.Members)
            {
                member.MenuVisibility = MemberActions(group, member).Count > 0 ? Visibility.Visible : Visibility.Collapsed;
            }
            GroupMembersList.ItemsSource = group.Members;

            LeaveGroupButton.Visibility = group is { Left: false, PendingInvite: false, MyRole: not null }
                ? Visibility.Visible
                : Visibility.Collapsed;
            GroupIdText.Text = "GroupID: " + group.GroupId;
        }
        finally
        {
            _updating = wasUpdating;
        }
        DispatcherQueue.TryEnqueue(() => ApplyFontToTree(GroupPage, _font.Family));
    }

    private static string RoleTitle(string? role) => role switch
    {
        "owner" => "вы владелец",
        "admin" => "вы администратор",
        _ => "вы участник",
    };

    /// <summary>Что текущий пользователь может сделать с участником. Старший по роли — больше.</summary>
    private List<(string Title, Func<Task> Run)> MemberActions(GroupViewModel group, GroupMemberModel member)
    {
        var actions = new List<(string Title, Func<Task> Run)>();
        if (member.IsSelf) return actions;
        actions.Add(("Написать лично", async () =>
        {
            GroupPage.Visibility = Visibility.Collapsed;
            if (member.IsContact)
            {
                await OpenChatAsync(member.UserId);
            }
            else
            {
                // Незнакомцу уходит обычный запрос на общение: текст в его публичный ящик.
                await ExecuteAsync(new { command = "add_contact", query = member.UserId, display_name = member.DisplayName });
            }
        }));
        if (group.CanManageAdmins && member.Role == "member")
        {
            actions.Add(("Назначить администратором", () => ExecuteAsync(new
            {
                command = "set_group_role", group_id = group.GroupId, user_id = member.UserId, role = "admin",
            })));
        }
        if (group.CanManageAdmins && member.Role == "admin")
        {
            actions.Add(("Снять права администратора", () => ExecuteAsync(new
            {
                command = "set_group_role", group_id = group.GroupId, user_id = member.UserId, role = "member",
            })));
        }
        if (group.CanManageAdmins && member.Role != "owner")
        {
            actions.Add(("Передать владение", async () =>
            {
                if (await ConfirmAsync("Передать владение?",
                        $"{member.DisplayName} станет владельцем группы, а вы — администратором. Вернуть владение сможет только новый владелец."))
                {
                    await ExecuteAsync(new { command = "transfer_group_ownership", group_id = group.GroupId, user_id = member.UserId });
                }
            }));
        }
        if (group.CanRemoveMembers && GroupMemberModel.Rank(member.Role) < GroupMemberModel.Rank(group.MyRole))
        {
            actions.Add(("Исключить из группы", async () =>
            {
                if (await ConfirmAsync("Исключить участника?",
                        $"{member.DisplayName} перестанет получать новые сообщения группы."))
                {
                    await ExecuteAsync(new { command = "remove_group_member", group_id = group.GroupId, user_id = member.UserId });
                }
            }));
        }
        return actions;
    }

    private MenuFlyout? BuildMemberMenu(GroupMemberModel member)
    {
        if (_snapshot?.Group is not GroupViewModel group) return null;
        List<(string Title, Func<Task> Run)> actions = MemberActions(group, member);
        if (actions.Count == 0) return null;
        var menu = new MenuFlyout();
        foreach ((string title, Func<Task> run) in actions)
        {
            menu.Items.Add(MenuItem(title, string.Empty, () => _ = run()));
        }
        return menu;
    }

    private void GroupMemberMenu_Click(object sender, RoutedEventArgs e)
    {
        if (sender is FrameworkElement { DataContext: GroupMemberModel member } element
            && BuildMemberMenu(member) is MenuFlyout menu)
        {
            menu.ShowAt(element);
        }
    }

    private void GroupMember_ContextRequested(UIElement sender, ContextRequestedEventArgs args)
    {
        if ((sender as FrameworkElement)?.DataContext is GroupMemberModel member && BuildMemberMenu(member) is MenuFlyout menu)
        {
            ShowMenu(menu, sender, args);
        }
    }

    private async void SaveGroupInfo_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.Group is not GroupViewModel group) return;
        string name = GroupNameInput.Text.Trim();
        string about = GroupAboutInput.Text.Trim();
        if (name == group.Name && about == group.About) return;
        _groupFormKey = null;
        await ExecuteAsync(new
        {
            command = "update_group_info", group_id = group.GroupId, name, about, avatar_base64 = group.AvatarBase64,
        });
    }

    private async void PickGroupAvatar_Click(object sender, RoutedEventArgs e)
    {
        StorageFile? file = await OpenFileAsync([".png", ".jpg", ".jpeg", ".bmp"]);
        if (file is null) return;
        // Сначала пробуем обычное качество, детальный снимок — ужимаем сильнее.
        string? encoded = await EncodeAvatarAsync(file, 160, 0.82);
        if (encoded is { Length: > GroupAvatarLimit }) encoded = await EncodeAvatarAsync(file, 112, 0.6);
        if (encoded is null)
        {
            await ShowErrorAsync("Не удалось прочитать изображение");
            return;
        }
        if (encoded.Length > GroupAvatarLimit)
        {
            await ShowErrorAsync("Фото слишком детальное для группы — выберите другое");
            return;
        }
        if (_snapshot?.Group is not GroupViewModel group) return;
        await ExecuteAsync(new
        {
            command = "update_group_info", group_id = group.GroupId, name = group.Name, about = group.About,
            avatar_base64 = encoded,
        });
    }

    private async void RemoveGroupAvatar_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.Group is not GroupViewModel group) return;
        await ExecuteAsync(new
        {
            command = "update_group_info", group_id = group.GroupId, name = group.Name, about = group.About,
            avatar_base64 = (string?)null,
        });
    }

    private async void GroupPermissions_Toggled(object sender, RoutedEventArgs e)
    {
        if (_updating || _snapshot?.Group is not GroupViewModel group) return;
        await ExecuteAsync(new
        {
            command = "set_group_permissions",
            group_id = group.GroupId,
            members_can_invite = MembersCanInviteToggle.IsOn,
            members_can_edit_info = MembersCanEditToggle.IsOn,
        });
    }

    private async void AddGroupMembers_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.Group is not GroupViewModel group) return;
        HashSet<string> present = [.. group.Members.Select(member => member.UserId)];
        List<ChatModel> candidates = [.. AcceptedContacts().Where(chat => !present.Contains(chat.UserId))];
        AddMembersContacts.ItemsSource = candidates;
        AddMembersEmpty.Visibility = candidates.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        AddMembersError.Visibility = Visibility.Collapsed;
        await TryShowDialogAsync(AddMembersDialog);
    }

    private async void AddMembersDialog_PrimaryButtonClick(ContentDialog sender, ContentDialogButtonClickEventArgs args)
    {
        if (_snapshot?.Group is not GroupViewModel group) return;
        string[] ids = [.. AddMembersContacts.SelectedItems.OfType<ChatModel>().Select(chat => chat.UserId)];
        if (ids.Length == 0)
        {
            args.Cancel = true;
            AddMembersError.Text = "Отметьте, кого добавить";
            AddMembersError.Visibility = Visibility.Visible;
            return;
        }
        ContentDialogButtonClickDeferral deferral = args.GetDeferral();
        bool added = await ExecuteAsync(new { command = "add_group_members", group_id = group.GroupId, user_ids = ids },
            showErrorDialog: false);
        args.Cancel = !added;
        if (!added)
        {
            AddMembersError.Text = _snapshot?.StatusMessage ?? "Не удалось добавить участников";
            AddMembersError.Visibility = Visibility.Visible;
        }
        deferral.Complete();
    }

    private async void LeaveGroup_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.SelectedChat is not ChatModel { IsGroup: true } chat) return;
        if (await ConfirmLeaveAsync(chat))
        {
            await ExecuteAsync(new { command = "leave_group", group_id = chat.UserId });
        }
    }

    // --- общие пункты меню чата -----------------------------------------------

    /// <summary>У группы и канала, кроме удаления, есть выход без потери истории.</summary>
    private void AddDeleteChatItems(MenuFlyout menu, ChatModel chat)
    {
        if (chat.IsChannel)
        {
            if (chat is { GroupLeft: false, PendingApproval: false } && chat.ChannelRole != "owner")
            {
                menu.Items.Add(MenuItem("Отписаться", string.Empty, async () =>
                {
                    if (await ConfirmAsync("Отписаться от канала?",
                            "История останется на этом устройстве, но новые посты приходить не будут."))
                        await ExecuteAsync(new { command = "leave_channel", channel_id = chat.UserId });
                }));
            }
            menu.Items.Add(MenuItem("Удалить канал", string.Empty, async () =>
            {
                if (await ConfirmDeleteChatAsync(chat))
                    await ExecuteAsync(new { command = "delete_contact", user_id = chat.UserId });
            }));
            return;
        }
        if (chat is { IsGroup: true, GroupLeft: false, PendingApproval: false })
        {
            menu.Items.Add(MenuItem("Покинуть группу", string.Empty, async () =>
            {
                if (await ConfirmLeaveAsync(chat))
                    await ExecuteAsync(new { command = "leave_group", group_id = chat.UserId });
            }));
        }
        menu.Items.Add(MenuItem(chat.IsGroup ? "Удалить группу" : "Удалить чат", string.Empty, async () =>
        {
            if (await ConfirmDeleteChatAsync(chat))
                await ExecuteAsync(new { command = "delete_contact", user_id = chat.UserId });
        }));
    }

    private Task<bool> ConfirmDeleteChatAsync(ChatModel chat) => chat.IsChannel
        ? ConfirmAsync("Удалить канал?", chat.ChannelRole == "owner" && !chat.GroupLeft
            ? "Вы владелец: сначала передайте канал другому администратору или удалите его у всех на странице канала."
            : chat.GroupLeft || chat.PendingApproval
                ? "История канала будет удалена с этого устройства."
                : "Вы отпишетесь, а история канала будет удалена с этого устройства.")
        : !chat.IsGroup
        ? ConfirmAsync("Удалить диалог?", "Локальная история этого диалога будет удалена.")
        : ConfirmAsync("Удалить группу?", chat.GroupLeft || chat.PendingApproval
            ? "История группы будет удалена с этого устройства."
            : "Вы покинете группу, а её история будет удалена с этого устройства.");

    private Task<bool> ConfirmLeaveAsync(ChatModel chat) => ConfirmAsync(
        "Покинуть группу?",
        (chat.GroupRole == "owner" ? "Владение перейдёт к старшему из оставшихся участников. " : string.Empty)
        + "История останется на этом устройстве, но новые сообщения приходить не будут.");

    /// <summary>WinUI запрещает два диалога сразу; второй просто не откроется, а не уронит процесс.</summary>
    private async Task TryShowDialogAsync(ContentDialog dialog)
    {
        try
        {
            await dialog.ShowAsync();
        }
        catch (Exception exception)
        {
            StatusText.Text = exception.Message;
        }
    }
}
