namespace TuratText.Client.Services;

public static class PlatformServices
{
    public static bool IsMobile { get; set; }
    public static IProtectedStorage? ProtectedStorage { get; set; }
}
