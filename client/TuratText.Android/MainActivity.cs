using Android.App;
using Android.Content.PM;

using Android.OS;
using Android.Runtime;
using Android.Views;
using AndroidX.Core.View;
using Avalonia;
using Avalonia.Android;
using TuratText.Client;
using TuratText.Client.Services;

namespace TuratText.Android;

[Activity(
    Label = "TuratText",
    Icon = "@mipmap/app_icon",
    Theme = "@style/TuratTextTheme",
    MainLauncher = true,
    WindowSoftInputMode = SoftInput.AdjustResize,
    ConfigurationChanges = ConfigChanges.Orientation | ConfigChanges.ScreenSize | ConfigChanges.UiMode)]
public sealed class MainActivity : AvaloniaMainActivity
{
    protected override void OnCreate(Bundle? savedInstanceState)
    {
        base.OnCreate(savedInstanceState);
        if (Window is null) return;

        // Draw behind the system bars and let Avalonia report the status bar, navigation bar and IME
        // as insets. The messenger view applies them itself, which is what lifts the composer above
        // the soft keyboard on Android 15+ where adjustResize no longer resizes the window.
        Window.SetSoftInputMode(SoftInput.AdjustResize);
        WindowCompat.SetDecorFitsSystemWindows(Window, false);

        // The app is dark, so keep the system bar icons light. Bar colours come from the theme,
        // which declares them transparent on every supported API level.
        View? decorView = Window.DecorView;
        WindowInsetsControllerCompat? controller = decorView is null
            ? null
            : WindowCompat.GetInsetsController(Window, decorView);
        if (controller is not null)
        {
            controller.AppearanceLightStatusBars = false;
            controller.AppearanceLightNavigationBars = false;
        }
    }
}

[Application]
public class AndroidApp : AvaloniaAndroidApplication<App>
{
    protected AndroidApp(nint javaReference, JniHandleOwnership transfer)
        : base(javaReference, transfer)
    {
    }

    protected override AppBuilder CustomizeAppBuilder(AppBuilder builder)
    {
        PlatformServices.IsMobile = true;
        PlatformServices.ProtectedStorage = new AndroidProtectedStorage(this);
        return base.CustomizeAppBuilder(builder).WithInterFont();
    }
}
