using Avalonia;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using TuratText.Client.LocalFirst;
using TuratText.Client.Messaging.V2;
using TuratText.Client.Services;
using TuratText.Client.Transport.V2;
using TuratText.Client.Updates;
using TuratText.Client.ViewModels;
using TuratText.Client.Views;

namespace TuratText.Client;

public partial class App : Application
{
    public override void Initialize()
    {
        AvaloniaXamlLoader.Load(this);
    }

    public override void OnFrameworkInitializationCompleted()
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            AppComposition composition = CreateComposition(isMobile: false);
            desktop.MainWindow = new MainWindow
            {
                DataContext = composition.Main
            };

            _ = BootstrapAsync(composition);
        }
        else if (ApplicationLifetime is IActivityApplicationLifetime activityLifetime)
        {
            AppComposition composition = CreateComposition(isMobile: true);
            activityLifetime.MainViewFactory = () => new MobileRootView
            {
                DataContext = composition.Main
            };

            _ = BootstrapAsync(composition);
        }
        else if (ApplicationLifetime is ISingleViewApplicationLifetime singleView)
        {
            AppComposition composition = CreateComposition(isMobile: true);
            singleView.MainView = new MobileRootView
            {
                DataContext = composition.Main
            };

            _ = BootstrapAsync(composition);
        }

        base.OnFrameworkInitializationCompleted();
    }

    private static AppComposition CreateComposition(bool isMobile)
    {
        PlatformServices.IsMobile = isMobile;
        IProtectedStorage protectedStorage = PlatformServices.ProtectedStorage
            ?? (isMobile
                ? throw new InvalidOperationException("Mobile protected storage is not configured")
                : new WindowsProtectedStorage());
        var protocolIdentity = new ProtocolIdentityService(protectedStorage);
        var localEvents = new LocalEventStore(protectedStorage);
        var localFirst = new LocalFirstRuntime(protocolIdentity, localEvents);
        var prekeys = new Crypto.V2.PrekeyStateService(protectedStorage, protocolIdentity);
        var deviceListV2 = new DeviceListService(protectedStorage, protocolIdentity);
        var deviceLinkV2 = new DeviceLinkService(protectedStorage, protocolIdentity, deviceListV2);
        var localBackupV2 = new LocalFirstBackupService(protectedStorage, localEvents);
        var relayDescriptors = new RelayDescriptorSource(protectedStorage);
        var metadataProtectionV2 = new MetadataProtectionSettings(protectedStorage);
        var connectivity = new ConnectivityHttpHandler(relayDescriptors, metadataProtectionV2);
        var v2Http = new HttpClient(connectivity)
        {
            Timeout = TimeSpan.FromSeconds(30),
            DefaultRequestVersion = System.Net.HttpVersion.Version20,
            DefaultVersionPolicy = HttpVersionPolicy.RequestVersionOrHigher
        };
        var mailboxTransport = new HttpsMailboxTransport(v2Http);
        var ownedMailboxes = new OwnedMailboxStore(protectedStorage);
        var routingDescriptors = new RoutingDescriptorService(protectedStorage, protocolIdentity, prekeys, deviceListV2);
        var prekeyNodeClient = new PrekeyNodeClient(v2Http);
        var routingNodeClient = new RoutingNodeClient(v2Http);
        var transparencyV2 = new TransparencyLogClient(v2Http, protectedStorage);
        var usernamesV2 = new UsernameDirectoryClient(v2Http, protectedStorage, protocolIdentity);
        var profilesV2 = new ProfileDirectoryClient(v2Http, protectedStorage, protocolIdentity);
        var updatesV2 = new ThresholdUpdateClient(v2Http, protectedStorage);
        var ownRoutingV2 = new OwnRoutingDescriptorStore(protectedStorage);
        var v2Provisioning = new MailboxProvisioningService(
            mailboxTransport,
            ownedMailboxes,
            protocolIdentity,
            prekeys,
            routingDescriptors,
            prekeyNodeClient,
            routingNodeClient,
            transparencyV2,
            ownRoutingV2);
        var bootstrapNodes = new BootstrapNodeSource(protectedStorage);
        var discoveryV2 = new DiscoveryBundleService(
            protocolIdentity,
            ownedMailboxes,
            bootstrapNodes,
            relayDescriptors);
        var contactsV2 = new LocalContactStore(protectedStorage);
        var privateMailboxGrantsV2 = new PrivateMailboxGrantStore(protectedStorage);
        var ratchetV2 = new Crypto.V2.RatchetSessionService(protectedStorage, protocolIdentity, prekeys);
        var attachmentsV2 = new Crypto.V2.EncryptedAttachmentService(v2Http);
        var messagingV2 = new V2MessagingService(
            localFirst,
            protocolIdentity,
            ratchetV2,
            ownedMailboxes,
            contactsV2,
            mailboxTransport,
            prekeyNodeClient,
            attachmentsV2,
            ownRoutingV2,
            privateMailboxGrantsV2,
            metadataProtectionV2);
        var portableEnvelopesV2 = new PortableEnvelopeService(localFirst, protocolIdentity, mailboxTransport);
        var main = new MainWindowViewModel(
            protocolIdentity,
            localFirst,
            v2Provisioning,
            bootstrapNodes,
            ownedMailboxes,
            routingNodeClient,
            usernamesV2,
            profilesV2,
            contactsV2,
            messagingV2,
            localBackupV2,
            deviceLinkV2,
            portableEnvelopesV2,
            discoveryV2,
            updatesV2,
            metadataProtectionV2,
            isMobile);
        return new AppComposition(main, localFirst);
    }

    private static async Task BootstrapAsync(AppComposition composition)
    {
        try
        {
            await composition.LocalFirst.InitializeAsync();
        }
        catch (Exception exception)
        {
            System.Diagnostics.Debug.WriteLine($"Local-first bootstrap failed: {exception}");
        }

        try
        {
            await composition.Main.ShowLocalFirstAsync(BootstrapNodeSource.DefaultBootstrapUrl);
        }
        catch (Exception exception)
        {
            System.Diagnostics.Debug.WriteLine($"Local-first messenger failed: {exception}");
        }
    }

    private sealed record AppComposition(
        MainWindowViewModel Main,
        LocalFirstRuntime LocalFirst);
}
