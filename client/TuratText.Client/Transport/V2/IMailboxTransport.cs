namespace TuratText.Client.Transport.V2;

public interface IMailboxTransport
{
    Task<NodeDescriptor> GetNodeDescriptorAsync(
        Uri bootstrapUri,
        string? expectedNodeId = null,
        CancellationToken cancellationToken = default);

    Task<OwnedMailboxRoute> RegisterMailboxAsync(
        NodeDescriptor node,
        string deviceHint,
        CancellationToken cancellationToken = default);

    Task PutAsync(
        PublicMailboxRoute route,
        MailboxEnvelope envelope,
        CancellationToken cancellationToken = default);

    Task<IReadOnlyList<MailboxEnvelope>> FetchAsync(
        OwnedMailboxRoute route,
        int limit = 100,
        CancellationToken cancellationToken = default);

    Task AcknowledgeAsync(
        OwnedMailboxRoute route,
        IReadOnlyCollection<string> envelopeIds,
        CancellationToken cancellationToken = default);
}
