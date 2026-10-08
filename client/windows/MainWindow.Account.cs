using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.ApplicationModel.DataTransfer;

namespace TuratText.Windows;

/// <summary>
/// Учётная запись: регистрация, вход по username и паролю, восстановление по ключу, ключ
/// восстановления, смена пароля, сеансы на других устройствах и выход. Файлы для переноса
/// между устройствами больше не нужны — новое устройство просто входит в аккаунт.
/// </summary>
public sealed partial class MainWindow
{
    private const int MinPasswordLength = 8;

    private enum AccountMode { Welcome, Register, Login, Recover, RecoveryKey, Conflict }

    private AccountMode _accountMode = AccountMode.Welcome;
    /// <summary>Переписку без аккаунта разрешено не защищать до следующего запуска.</summary>
    private bool _accountPostponed;
    private bool _conflictPostponed;

    private static string DeviceName => $"Windows · {Environment.MachineName}";

    /// <summary>
    /// Что показать поверх окна. Порядок важен: ключ восстановления — сначала, потом вход,
    /// потом напоминания, которые можно отложить.
    /// </summary>
    private void UpdateAccountPage()
    {
        if (_snapshot is null) return;
        AccountModel account = _snapshot.Account;
        AccountMode? required = account switch
        {
            { RecoveryKey: not null } => AccountMode.RecoveryKey,
            { State: "none" } => _accountMode is AccountMode.Register or AccountMode.Login or AccountMode.Recover
                ? _accountMode
                : AccountMode.Welcome,
            { State: "legacy" } when !_accountPostponed => _accountMode is AccountMode.Login or AccountMode.Recover
                ? _accountMode
                : AccountMode.Register,
            { UsernameConflict: true } when !_conflictPostponed => AccountMode.Conflict,
            _ => null,
        };
        if (required is null)
        {
            AccountPage.Visibility = Visibility.Collapsed;
            return;
        }
        bool opening = AccountPage.Visibility != Visibility.Visible || _accountMode != required;
        _accountMode = required.Value;
        AccountPage.Visibility = Visibility.Visible;
        if (opening) ShowAccountMode(_accountMode);
    }

    private void ShowAccountMode(AccountMode mode)
    {
        if (_snapshot is null) return;
        _accountMode = mode;
        AccountModel account = _snapshot.Account;
        bool legacy = account.State == "legacy";
        AccountWelcomePanel.Visibility = Show(mode == AccountMode.Welcome);
        AccountRegisterPanel.Visibility = Show(mode == AccountMode.Register);
        AccountLoginPanel.Visibility = Show(mode == AccountMode.Login);
        AccountRecoverPanel.Visibility = Show(mode == AccountMode.Recover);
        AccountRecoveryKeyPanel.Visibility = Show(mode == AccountMode.RecoveryKey);
        AccountConflictPanel.Visibility = Show(mode == AccountMode.Conflict);
        AccountError.Visibility = Visibility.Collapsed;

        (AccountTitle.Text, AccountDescription.Text) = mode switch
        {
            AccountMode.Welcome => ("Turat",
                "Мессенджер со сквозным шифрованием. Аккаунт не привязан ни к телефону, ни к почте — только username и пароль."),
            AccountMode.Register when legacy => ("Защитите аккаунт",
                "Придумайте пароль — с ним вы войдёте в Turat на любом устройстве, а переписка будет синхронизироваться сама. Ваши чаты останутся на месте."),
            AccountMode.Register => ("Новый аккаунт",
                "Username — это и ваш адрес для собеседников, и логин. Пароль никуда не отправляется: Node хранит только зашифрованные данные."),
            AccountMode.Login => ("Вход", "Введите username и пароль. История переписки загрузится с ваших устройств."),
            AccountMode.Recover => ("Восстановление доступа",
                "Введите ключ восстановления, который вы сохранили при регистрации, и придумайте новый пароль."),
            AccountMode.RecoveryKey => ("Ключ восстановления",
                "Это единственный способ вернуть доступ к аккаунту, если вы забудете пароль. Ни Turat, ни Node не знают ни ключа, ни пароля и восстановить их не смогут."),
            AccountMode.Conflict => ("Выберите новый username",
                $"Username @{account.Username} на этом Node уже занят другим человеком. Придумайте другой — по нему вас найдут собеседники, и с ним вы будете входить в аккаунт."),
            _ => ("Turat", string.Empty),
        };

        bool notice = account.Notice is not null && mode is AccountMode.Welcome or AccountMode.Login;
        AccountNotice.Visibility = Show(notice);
        AccountNoticeText.Text = account.Notice ?? string.Empty;

        if (mode == AccountMode.Register)
        {
            RegisterDisplayName.Text = _snapshot.Profile.DisplayName;
            RegisterDisplayName.Visibility = Show(!legacy || _snapshot.Profile.DisplayName.Length == 0);
            if (RegisterUsername.Text.Length == 0) RegisterUsername.Text = _snapshot.Profile.Username;
            RegisterButton.Content = "Создать аккаунт";
        }
        if (mode == AccountMode.RecoveryKey)
        {
            string key = account.RecoveryKey ?? string.Empty;
            string[] groups = key.Split('-');
            RecoveryKeyText.Text = groups.Length == 8
                ? string.Join("  ", groups[..4]) + "\n" + string.Join("  ", groups[4..])
                : key;
            RecoveryKeySaved.IsChecked = false;
            RecoveryKeyDoneButton.IsEnabled = false;
            CopyRecoveryKeyButton.Content = "Скопировать ключ";
        }

        AccountBackLink.Visibility = Show(!legacy && mode is AccountMode.Register or AccountMode.Login or AccountMode.Recover);
        AccountOtherLink.Visibility = Show(legacy && mode is AccountMode.Register or AccountMode.Login or AccountMode.Recover);
        AccountOtherLink.Content = mode == AccountMode.Register ? "Войти в другой аккаунт" : "Создать аккаунт для этой переписки";
        AccountLaterLink.Visibility = Show(legacy && mode != AccountMode.RecoveryKey || mode == AccountMode.Conflict);
        AccountLaterLink.Content = mode == AccountMode.Conflict ? "Позже" : "Напомнить позже";
        AccountNodePanel.Visibility = Show(mode is AccountMode.Welcome or AccountMode.Register or AccountMode.Login or AccountMode.Recover);
        AccountNodeText.Text = "Node: " + NodeHost(_snapshot.Settings.BootstrapUrl);
    }

    private static string NodeHost(string url) =>
        Uri.TryCreate(url, UriKind.Absolute, out Uri? parsed) ? parsed.Authority : url;

    private void AccountNodeEdit_Click(object sender, RoutedEventArgs e)
    {
        AccountNodeInput.Text = _snapshot?.Settings.BootstrapUrl ?? string.Empty;
        AccountNodeEditor.Visibility = AccountNodeEditor.Visibility == Visibility.Visible
            ? Visibility.Collapsed
            : Visibility.Visible;
    }

    private async void AccountNodeConnect_Click(object sender, RoutedEventArgs e)
    {
        string url = AccountNodeInput.Text.Trim();
        if (url.Length == 0) return;
        if (!url.Contains("://", StringComparison.Ordinal)) url = "https://" + url;
        if (await AccountCommandAsync(new { command = "connect", bootstrap_url = url }))
        {
            AccountNodeEditor.Visibility = Visibility.Collapsed;
            if (_snapshot is not null) AccountNodeText.Text = "Node: " + NodeHost(_snapshot.Settings.BootstrapUrl);
        }
    }

    private static Visibility Show(bool visible) => visible ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Команда аккаунта: ошибку показываем под формой, а не отдельным окном.</summary>
    private async Task<bool> AccountCommandAsync(object command)
    {
        AccountError.Visibility = Visibility.Collapsed;
        AccountBusy.Visibility = Visibility.Visible;
        try
        {
            bool ok = await ExecuteAsync(command, showErrorDialog: false);
            if (!ok)
            {
                AccountError.Text = _snapshot?.StatusMessage is { Length: > 0 } message
                    ? message
                    : "Не получилось. Попробуйте ещё раз.";
                AccountError.Visibility = Visibility.Visible;
            }
            return ok;
        }
        finally
        {
            AccountBusy.Visibility = Visibility.Collapsed;
        }
    }

    private void ShowAccountError(string message)
    {
        AccountError.Text = message;
        AccountError.Visibility = Visibility.Visible;
    }

    private void AccountShowWelcome_Click(object sender, RoutedEventArgs e) => ShowAccountMode(AccountMode.Welcome);
    private void AccountShowRegister_Click(object sender, RoutedEventArgs e) => ShowAccountMode(AccountMode.Register);
    private void AccountShowLogin_Click(object sender, RoutedEventArgs e) => ShowAccountMode(AccountMode.Login);
    private void AccountShowRecover_Click(object sender, RoutedEventArgs e) => ShowAccountMode(AccountMode.Recover);

    private void AccountOther_Click(object sender, RoutedEventArgs e) =>
        ShowAccountMode(_accountMode == AccountMode.Register ? AccountMode.Login : AccountMode.Register);

    private void AccountLater_Click(object sender, RoutedEventArgs e)
    {
        if (_accountMode == AccountMode.Conflict) _conflictPostponed = true;
        else _accountPostponed = true;
        UpdateAccountPage();
    }

    private async void AccountRegister_Click(object sender, RoutedEventArgs e)
    {
        if (RegisterPassword.Password.Length < MinPasswordLength)
        {
            ShowAccountError($"Пароль должен содержать минимум {MinPasswordLength} символов");
            return;
        }
        if (RegisterPassword.Password != RegisterPasswordRepeat.Password)
        {
            ShowAccountError("Пароли не совпадают");
            return;
        }
        RegisterButton.Content = "Создаём…";
        bool ok = await AccountCommandAsync(new
        {
            command = "account_register",
            username = RegisterUsername.Text.Trim(),
            display_name = RegisterDisplayName.Text.Trim(),
            password = RegisterPassword.Password,
            device_name = DeviceName,
        });
        RegisterButton.Content = "Создать аккаунт";
        if (ok)
        {
            RegisterPassword.Password = RegisterPasswordRepeat.Password = string.Empty;
            ScheduleNextSync();
        }
    }

    private void LoginPassword_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == global::Windows.System.VirtualKey.Enter) AccountLogin_Click(sender, e);
    }

    private async void AccountLogin_Click(object sender, RoutedEventArgs e)
    {
        if (LoginUsername.Text.Trim().Length == 0 || LoginPassword.Password.Length == 0)
        {
            ShowAccountError("Введите username и пароль");
            return;
        }
        bool legacy = _snapshot?.Account.State == "legacy";
        if (legacy && !await ConfirmDiscardAsync()) return;
        if (await AccountCommandAsync(new
        {
            command = "account_login",
            username = LoginUsername.Text.Trim(),
            password = LoginPassword.Password,
            device_name = DeviceName,
            discard_local = legacy,
        }))
        {
            LoginPassword.Password = string.Empty;
            ScheduleNextSync();
        }
    }

    private async void AccountRecover_Click(object sender, RoutedEventArgs e)
    {
        if (RecoverPassword.Password.Length < MinPasswordLength)
        {
            ShowAccountError($"Пароль должен содержать минимум {MinPasswordLength} символов");
            return;
        }
        if (RecoverPassword.Password != RecoverPasswordRepeat.Password)
        {
            ShowAccountError("Пароли не совпадают");
            return;
        }
        bool legacy = _snapshot?.Account.State == "legacy";
        if (legacy && !await ConfirmDiscardAsync()) return;
        if (await AccountCommandAsync(new
        {
            command = "account_recover",
            recovery_key = RecoverKey.Text,
            new_password = RecoverPassword.Password,
            device_name = DeviceName,
            discard_local = legacy,
        }))
        {
            RecoverKey.Text = RecoverPassword.Password = RecoverPasswordRepeat.Password = string.Empty;
            ScheduleNextSync();
        }
    }

    private Task<bool> ConfirmDiscardAsync() => ConfirmAsync(
        "Удалить переписку на этом устройстве?",
        "На этом устройстве есть чаты без аккаунта. При входе в другой аккаунт они будут удалены без возможности восстановления.");

    private void CopyRecoveryKey_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot?.Account.RecoveryKey is not { } key) return;
        var package = new DataPackage();
        package.SetText(key);
        Clipboard.SetContent(package);
        CopyRecoveryKeyButton.Content = "Скопировано";
    }

    private void RecoveryKeySaved_Changed(object sender, RoutedEventArgs e) =>
        RecoveryKeyDoneButton.IsEnabled = RecoveryKeySaved.IsChecked == true;

    private async void RecoveryKeyDone_Click(object sender, RoutedEventArgs e) =>
        await AccountCommandAsync(new { command = "account_confirm_recovery_key" });

    private async void ConflictSave_Click(object sender, RoutedEventArgs e)
    {
        if (_snapshot is null) return;
        ProfileModel profile = _snapshot.Profile;
        if (await AccountCommandAsync(new
        {
            command = "save_profile",
            username = ConflictUsername.Text.Trim(),
            display_name = profile.DisplayName,
            about = profile.About,
            avatar_base64 = profile.AvatarBase64,
        }))
        {
            await AccountCommandAsync(new { command = "publish_profile" });
        }
    }

    // --- настройки: раздел «Аккаунт» ------------------------------------------

    private void FillAccountSettings()
    {
        if (_snapshot is null) return;
        AccountModel account = _snapshot.Account;
        bool signedIn = account.State == "active";
        AccountManagePanel.Visibility = Show(signedIn);
        SetUpAccountButton.Visibility = Show(!signedIn);
        AccountSummary.Text = signedIn
            ? $"@{account.Username} — это и логин. Аккаунт хранится на {account.Node.Replace("https://", string.Empty)}."
            : "У этой переписки ещё нет аккаунта. Создайте его — тогда вы сможете войти на другом устройстве по username и паролю, а чаты будут синхронизироваться.";
        if (!signedIn) return;

        DevicesPanel.Children.Clear();
        foreach (AccountDeviceModel device in account.Devices)
        {
            var row = new Grid { ColumnSpacing = 10, Padding = new Thickness(0, 6, 0, 6) };
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
            row.ColumnDefinitions.Add(new ColumnDefinition { Width = GridLength.Auto });
            var info = new StackPanel();
            info.Children.Add(new TextBlock
            {
                Text = device.Name,
                FontWeight = Microsoft.UI.Text.FontWeights.SemiBold,
                Foreground = (Brush)Application.Current.Resources["TgText"],
            });
            info.Children.Add(new TextBlock
            {
                Text = device.Current
                    ? "это устройство"
                    : "вход " + DateTimeOffset.FromUnixTimeMilliseconds(device.AddedAtUnixMilliseconds).LocalDateTime.ToString("d MMMM yyyy"),
                FontSize = 13,
                Foreground = (Brush)Application.Current.Resources[device.Current ? "TgAccent" : "TgHint"],
            });
            row.Children.Add(info);
            if (!device.Current)
            {
                var revoke = new Button
                {
                    Content = "Завершить сеанс",
                    Foreground = (Brush)Application.Current.Resources["TgDanger"],
                    VerticalAlignment = VerticalAlignment.Center,
                };
                string deviceId = device.DeviceId;
                string name = device.Name;
                revoke.Click += async (_, _) => await RevokeDeviceAsync(deviceId, name);
                Grid.SetColumn(revoke, 1);
                row.Children.Add(revoke);
            }
            DevicesPanel.Children.Add(row);
        }
        if (!ReferenceEquals(FocusManager.GetFocusedElement(Root.XamlRoot), DeviceNameInput))
        {
            DeviceNameInput.Text = account.Devices.FirstOrDefault(device => device.Current)?.Name ?? string.Empty;
        }
    }

    private async Task RevokeDeviceAsync(string deviceId, string name)
    {
        if (!await ConfirmAsync("Завершить сеанс?",
                $"«{name}» выйдет из аккаунта, переписка на нём будет удалена, и новые сообщения туда приходить перестанут.")) return;
        await ExecuteAsync(new { command = "revoke_device", device_id = deviceId });
    }

    private void SetUpAccount_Click(object sender, RoutedEventArgs e)
    {
        _accountPostponed = false;
        SettingsPage.Visibility = Visibility.Collapsed;
        _accountMode = AccountMode.Register;
        UpdateAccountPage();
        ShowAccountMode(AccountMode.Register);
    }

    private async void ChangePassword_Click(object sender, RoutedEventArgs e)
    {
        if (NewPasswordInput.Password.Length < MinPasswordLength)
        {
            await ShowErrorAsync($"Новый пароль должен содержать минимум {MinPasswordLength} символов");
            return;
        }
        if (NewPasswordInput.Password != NewPasswordRepeatInput.Password)
        {
            await ShowErrorAsync("Новые пароли не совпадают");
            return;
        }
        if (await ExecuteAsync(new
        {
            command = "account_change_password",
            old_password = CurrentPasswordInput.Password,
            new_password = NewPasswordInput.Password,
        }))
        {
            CurrentPasswordInput.Password = NewPasswordInput.Password = NewPasswordRepeatInput.Password = string.Empty;
        }
    }

    private async void NewRecoveryKey_Click(object sender, RoutedEventArgs e)
    {
        if (RecoveryPasswordInput.Password.Length == 0)
        {
            await ShowErrorAsync("Введите пароль");
            return;
        }
        if (await ExecuteAsync(new { command = "account_new_recovery_key", password = RecoveryPasswordInput.Password }))
        {
            RecoveryPasswordInput.Password = string.Empty;
            SettingsPage.Visibility = Visibility.Collapsed;
            UpdateAccountPage();
        }
    }

    private async void RenameDevice_Click(object sender, RoutedEventArgs e) =>
        await ExecuteAsync(new { command = "account_rename_device", name = DeviceNameInput.Text.Trim() });

    private async void Logout_Click(object sender, RoutedEventArgs e)
    {
        if (!await ConfirmAsync("Выйти из аккаунта?",
                "Все чаты и файлы на этом устройстве будут удалены. На других ваших устройствах всё останется, а войти снова можно по username и паролю.")) return;
        if (await ExecuteAsync(new { command = "account_logout" }))
        {
            SettingsPage.Visibility = Visibility.Collapsed;
            _accountMode = AccountMode.Welcome;
            UpdateAccountPage();
        }
    }
}
