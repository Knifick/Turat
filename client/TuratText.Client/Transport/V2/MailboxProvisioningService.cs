using TuratText.Client.Crypto.V2;
using TuratText.Client.LocalFirst;

namespace TuratText.Client.Transport.V2;

public sealed class MailboxProvisioningService
{
    private readonly IMailboxTransport _transport;
    private readonly OwnedMailboxStore _store;
    private readonly ProtocolIdentityService _identity;
    private readonly PrekeyStateService _prekeys;
    private readonly RoutingDescriptorService _routing;
    private readonly PrekeyNodeClient _prekeyClient;
    private readonly RoutingNodeClient _routingClient;
    private readonly TransparencyLogClient _transparency;
    private readonly OwnRoutingDescriptorStore _ownRouting;

    public MailboxProvisioningService(
        IMailboxTransport transport,
        OwnedMailboxStore store,
        ProtocolIdentityService identity,
        PrekeyStateService prekeys,
        RoutingDescriptorService routing,
        PrekeyNodeClient prekeyClient,
        RoutingNodeClient routingClient,
        TransparencyLogClient transparency,
        OwnRoutingDescriptorStore ownRouting)
    {
        _transport = transport;
        _store = store;
        _identity = identity;
        _prekeys = prekeys;
        _routing = routing;
        _prekeyClient = prekeyClient;
        _routingClient = routingClient;
        _transparency = transparency;
        _ownRouting = ownRouting;
    }

    public async Task<IReadOnlyList<OwnedMailboxRoute>> ProvisionAsync(
        IEnumerable<(Uri Uri, string? ExpectedNodeId)> bootstrapNodes,
        CancellationToken cancellationToken = default)
    {
        ProtocolIdentity identity = await _identity.GetOrCreateAsync(cancellationToken);
        List<OwnedMailboxRoute> routes = (await _store.LoadAsync(cancellationToken)).ToList();
        foreach ((Uri uri, string? expectedNodeId) in bootstrapNodes)
        {
            NodeDescriptor node = await _transport.GetNodeDescriptorAsync(uri, expectedNodeId, cancellationToken);
            OwnedMailboxRoute? existing = routes.FirstOrDefault(route =>
                route.Node.NodeId == node.NodeId && route.ExpiresAt > DateTimeOffset.UtcNow.AddDays(1));
            if (existing is null)
            {
                existing = await _transport.RegisterMailboxAsync(node, identity.DeviceId, cancellationToken);
                routes.RemoveAll(route => route.Node.NodeId == node.NodeId);
                routes.Add(existing);
            }
            PrekeyPublication publication = await _prekeys.GetOrCreatePublicationForNodeAsync(
                node.NodeId,
                cancellationToken: cancellationToken);
            await _prekeyClient.PublishAsync(existing, publication, cancellationToken);
        }
        await _store.SaveAsync(routes, cancellationToken);
        SignedRoutingDescriptor? previous = null;
        foreach (OwnedMailboxRoute route in routes)
        {
            try
            {
                SignedRoutingDescriptor candidate = await _routingClient.GetAsync(
                    route.Node,
                    identity.UserId,
                    cancellationToken);
                if (previous is null || candidate.Descriptor.Sequence > previous.Descriptor.Sequence)
                    previous = candidate;
            }
            catch (HttpRequestException)
            {
                // A fresh node does not have this identity yet.
            }
        }
        SignedRoutingDescriptor descriptor = await _routing.CreateAsync(routes, previous, cancellationToken);
        foreach (OwnedMailboxRoute route in routes)
        {
            await _routingClient.PublishAsync(route.Node, descriptor, cancellationToken);
            await _transparency.SyncAsync(route.Node, cancellationToken);
        }
        await _ownRouting.SaveAsync(descriptor, cancellationToken);
        return routes;
    }
}
