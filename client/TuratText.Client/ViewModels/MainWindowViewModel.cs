using TuratText.Client.LocalFirst;
using TuratText.Client.Messaging.V2;
using TuratText.Client.Transport.V2;
using TuratText.Client.Updates;

namespace TuratText.Client.ViewModels;

public sealed class MainWindowViewModel : ViewModelBase
{
    private readonly ProtocolIdentityService _protocolIdentity;
    private readonly LocalFirstRuntime _localFirst;
    private readonly MailboxProvisioningService _v2Provisioning;
    private readonly BootstrapNodeSource _bootstrapNodes;
    private readonly OwnedMailboxStore _ownedMailboxes;
    private readonly RoutingNodeClient _routingNodeClient;
    private readonly UsernameDirectoryClient _usernamesV2;
    private readonly ProfileDirectoryClient _profilesV2;
    private readonly LocalContactStore _contactsV2;
    private readonly V2MessagingService _messagingV2;
    private readonly LocalFirstBackupService _backupV2;
    private readonly DeviceLinkService _deviceLinkV2;
    private readonly PortableEnvelopeService _portableEnvelopesV2;
    private readonly DiscoveryBundleService _discoveryV2;
    private readonly ThresholdUpdateClient _updatesV2;
    private readonly MetadataProtectionSettings _metadataProtectionV2;

    private ViewModelBase? _currentViewModel;

    public MainWindowViewModel(
        ProtocolIdentityService protocolIdentity,
        LocalFirstRuntime localFirst,
        MailboxProvisioningService v2Provisioning,
        BootstrapNodeSource bootstrapNodes,
        OwnedMailboxStore ownedMailboxes,
        RoutingNodeClient routingNodeClient,
        UsernameDirectoryClient usernamesV2,
        ProfileDirectoryClient profilesV2,
        LocalContactStore contactsV2,
        V2MessagingService messagingV2,
        LocalFirstBackupService backupV2,
        DeviceLinkService deviceLinkV2,
        PortableEnvelopeService portableEnvelopesV2,
        DiscoveryBundleService discoveryV2,
        ThresholdUpdateClient updatesV2,
        MetadataProtectionSettings metadataProtectionV2,
        bool isMobile = false)
    {
        _protocolIdentity = protocolIdentity;
        _localFirst = localFirst;
        _v2Provisioning = v2Provisioning;
        _bootstrapNodes = bootstrapNodes;
        _ownedMailboxes = ownedMailboxes;
        _routingNodeClient = routingNodeClient;
        _usernamesV2 = usernamesV2;
        _profilesV2 = profilesV2;
        _contactsV2 = contactsV2;
        _messagingV2 = messagingV2;
        _backupV2 = backupV2;
        _deviceLinkV2 = deviceLinkV2;
        _portableEnvelopesV2 = portableEnvelopesV2;
        _discoveryV2 = discoveryV2;
        _updatesV2 = updatesV2;
        _metadataProtectionV2 = metadataProtectionV2;
        IsMobile = isMobile;
        _currentViewModel = null;
    }

    public bool IsMobile { get; }

    public ViewModelBase? CurrentViewModel
    {
        get => _currentViewModel;
        private set => SetProperty(ref _currentViewModel, value);
    }

    public async Task ShowLocalFirstAsync(string? bootstrapUrl = null)
    {
        var messenger = new LocalFirstMessengerViewModel(
            this,
            _protocolIdentity,
            _localFirst,
            _v2Provisioning,
            _bootstrapNodes,
            _ownedMailboxes,
            _routingNodeClient,
            _usernamesV2,
            _profilesV2,
            _contactsV2,
            _messagingV2,
            _backupV2,
            _deviceLinkV2,
            _portableEnvelopesV2,
            _discoveryV2,
            _updatesV2,
            _metadataProtectionV2,
            bootstrapUrl);
        CurrentViewModel = messenger;
        await messenger.InitializeAsync();
    }

}
