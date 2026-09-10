using TuratText.Windows.Interop;

namespace TuratText.Windows;

internal static class Program
{
    [STAThread]
    private static int Main(string[] args)
    {
        if (args.Contains("--core-smoke", StringComparer.Ordinal))
        {
            try
            {
                using var core = new RustCore();
                CoreResponse response = core.Invoke(new { command = "snapshot" });
                return response.Ok && response.Snapshot?.Identity.UserId.StartsWith("tt1-", StringComparison.Ordinal) == true
                    ? 0
                    : 2;
            }
            catch
            {
                return 3;
            }
        }

        XamlGeneratedProgram.XamlGeneratedMain();
        return 0;
    }
}
