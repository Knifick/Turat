namespace TuratText.Client.LocalFirst;

public sealed class LocalFirstRuntime : IAsyncDisposable, IDisposable
{
    private readonly ProtocolIdentityService _identities;
    private readonly LocalEventStore _events;

    public LocalFirstRuntime(ProtocolIdentityService identities, LocalEventStore events)
    {
        _identities = identities;
        _events = events;
    }

    public ProtocolIdentity Identity => _identities.Current
                                        ?? throw new InvalidOperationException("Local-first runtime is not initialized");

    public LocalEventStore Events => _events;

    public async Task InitializeAsync(CancellationToken cancellationToken = default)
    {
        await _identities.GetOrCreateAsync(cancellationToken);
        await _events.InitializeAsync(cancellationToken);
        await _events.PruneProcessedEnvelopesAsync(cancellationToken);
    }

    public async Task<SignedProtocolEvent> CreateAndQueueEventAsync(
        string conversationId,
        string kind,
        ReadOnlyMemory<byte> opaquePayload,
        CancellationToken cancellationToken = default)
    {
        long sequence = await _events.ReserveNextDeviceSequenceAsync(Identity.DeviceId, cancellationToken);
        SignedProtocolEvent value = _identities.SignEvent(
            conversationId,
            kind,
            opaquePayload.Span,
            sequence);
        if (!await _events.AppendAsync(value, EventDeliveryState.Pending, cancellationToken))
        {
            throw new InvalidOperationException("A locally generated event was rejected as a duplicate");
        }
        return value;
    }

    public async Task<bool> AcceptIncomingEventAsync(
        SignedProtocolEvent value,
        ProtocolIdentity sender,
        string envelopeId,
        DateTimeOffset envelopeExpiresAt,
        CancellationToken cancellationToken = default)
    {
        if (!ProtocolIdentityService.VerifyEvent(value, sender))
        {
            throw new InvalidOperationException("Incoming event signature is invalid");
        }
        return await _events.AppendIncomingAsync(
            value,
            envelopeId,
            envelopeExpiresAt,
            cancellationToken);
    }

    public void Dispose() => _identities.Dispose();

    public async ValueTask DisposeAsync()
    {
        _identities.Dispose();
        await _events.DisposeAsync();
    }
}
