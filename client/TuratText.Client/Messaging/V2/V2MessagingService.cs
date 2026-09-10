using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Avalonia;
using Avalonia.Layout;
using Avalonia.Media;
using TuratText.Client.Crypto.V2;
using TuratText.Client.LocalFirst;
using TuratText.Client.Transport.V2;

namespace TuratText.Client.Messaging.V2;

public sealed record LocalTextMessage(
    string EventId,
    string SenderUserId,
    string Text,
    DateTimeOffset CreatedAt,
    bool Outgoing,
    bool Edited,
    bool Deleted,
    IReadOnlyList<string> Reactions,
    bool Delivered,
    bool Read,
    EncryptedAttachmentManifest? Attachment)
{
    public string ReactionSummary => string.Join(" ", Reactions);
    public string DisplayText => Deleted ? "Сообщение удалено" : Text;
    public string TimeLabel => CreatedAt.ToLocalTime().ToString("HH:mm");
    public bool HasAttachment => Attachment is not null;
    public bool HasReactions => Reactions.Count > 0;
    public bool CanModify => Outgoing && !Deleted;
}

public sealed class V2MessagingService
{
    private const int MaxMessageUtf8Bytes = 64 * 1024;
    private const int MaxContactRequestUtf8Bytes = 4 * 1024;
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly LocalFirstRuntime _runtime;
    private readonly ProtocolIdentityService _identity;
    private readonly RatchetSessionService _ratchet;
    private readonly OwnedMailboxStore _mailboxes;
    private readonly LocalContactStore _contacts;
    private readonly IMailboxTransport _transport;
    private readonly PrekeyNodeClient _prekeyClient;
    private readonly EncryptedAttachmentService _attachments;
    private readonly OwnRoutingDescriptorStore _ownRouting;
    private readonly PrivateMailboxGrantStore _privateGrants;
    private readonly MetadataProtectionSettings _metadataProtection;
    private readonly SemaphoreSlim _deliveryGate = new(1, 1);

    public V2MessagingService(
        LocalFirstRuntime runtime,
        ProtocolIdentityService identity,
        RatchetSessionService ratchet,
        OwnedMailboxStore mailboxes,
        LocalContactStore contacts,
        IMailboxTransport transport,
        PrekeyNodeClient prekeyClient,
        EncryptedAttachmentService attachments,
        OwnRoutingDescriptorStore ownRouting,
        PrivateMailboxGrantStore privateGrants,
        MetadataProtectionSettings metadataProtection)
    {
        _runtime = runtime;
        _identity = identity;
        _ratchet = ratchet;
        _mailboxes = mailboxes;
        _contacts = contacts;
        _transport = transport;
        _prekeyClient = prekeyClient;
        _attachments = attachments;
        _ownRouting = ownRouting;
        _privateGrants = privateGrants;
        _metadataProtection = metadataProtection;
    }

    public string CurrentUserId => _identity.Current?.UserId ?? "";
    public string? LastFetchError { get; private set; }
    public string? LastFlushError { get; private set; }

    /// <summary>
    /// Raised when a presence heartbeat arrives from a contact: userId and the timestamp they sent
    /// it at (treat as "online" while fresh, otherwise as that contact's last-seen time). Fired from
    /// whatever thread called <see cref="FetchAsync"/> — marshal back to the UI thread yourself.
    /// </summary>
    public event Action<string, DateTimeOffset>? PresenceReceived;

    public Task<IReadOnlyList<LocalContact>> ContactsAsync(CancellationToken cancellationToken = default) =>
        _contacts.LoadAsync(cancellationToken);

    public async Task<SignedProtocolEvent> SendTextAsync(
        LocalContact contact,
        string text,
        CancellationToken cancellationToken = default)
    {
        if (string.IsNullOrWhiteSpace(text)) throw new ArgumentException("Message text is required", nameof(text));
        if (Encoding.UTF8.GetByteCount(text.Trim()) > MaxMessageUtf8Bytes)
            throw new ArgumentException("Message text is too large", nameof(text));
        return await SendPayloadAsync(
            contact,
            "message.text",
            new TextPayload(2, text.Trim()),
            cancellationToken);
    }

    public Task<SignedProtocolEvent> EditMessageAsync(
        LocalContact contact,
        string targetEventId,
        string text,
        CancellationToken cancellationToken = default) =>
        SendPayloadAsync(contact, "message.edit", new EditPayload(2, targetEventId, text.Trim()), cancellationToken);

    public Task<SignedProtocolEvent> DeleteMessageAsync(
        LocalContact contact,
        string targetEventId,
        CancellationToken cancellationToken = default) =>
        SendPayloadAsync(contact, "message.delete", new TargetPayload(2, targetEventId), cancellationToken);

    /// <summary>
    /// Sends a best-effort "I'm here" heartbeat over the same E2EE mailbox path as regular messages.
    /// Rides the existing delivery/ratchet/outbox machinery, but the receiving side discards it from
    /// local storage immediately after reading the timestamp — it's a live signal, not history.
    /// </summary>
    public Task<SignedProtocolEvent> SendPresenceAsync(
        LocalContact contact,
        CancellationToken cancellationToken = default) =>
        SendPayloadAsync(
            contact,
            "presence.update",
            new PresencePayload(2, DateTimeOffset.UtcNow.ToUnixTimeMilliseconds()),
            cancellationToken);

    public Task<SignedProtocolEvent> SetReactionAsync(
        LocalContact contact,
        string targetEventId,
        string reaction,
        bool active,
        CancellationToken cancellationToken = default) =>
        SendPayloadAsync(
            contact,
            "message.reaction",
            new ReactionPayload(2, targetEventId, reaction, active),
            cancellationToken);

    public async Task<SignedProtocolEvent> SendAttachmentAsync(
        LocalContact contact,
        ReadOnlyMemory<byte> content,
        string fileName,
        string mimeType,
        string? caption = null,
        CancellationToken cancellationToken = default)
    {
        if (contact.PendingApproval || !await HasPrivateDeliveryForEveryDeviceAsync(contact, cancellationToken))
            throw new InvalidOperationException("Attachments require an accepted contact and a private mailbox capability");
        IReadOnlyList<NodeDescriptor> nodes = (await _mailboxes.LoadAsync(cancellationToken))
            .Select(value => value.Node)
            .DistinctBy(value => value.NodeId)
            .ToList();
        EncryptedAttachmentManifest manifest = await _attachments.EncryptAndUploadAsync(
            content,
            fileName,
            mimeType,
            nodes,
            cancellationToken);
        return await SendPayloadAsync(
            contact,
            "message.attachment",
            new AttachmentPayload(2, caption?.Trim() ?? "", manifest),
            cancellationToken);
    }

    public Task<byte[]> DownloadAttachmentAsync(
        EncryptedAttachmentManifest manifest,
        CancellationToken cancellationToken = default) =>
        _attachments.DownloadAndDecryptAsync(manifest, cancellationToken);

    private async Task<bool> HasPrivateDeliveryForEveryDeviceAsync(
        LocalContact contact,
        CancellationToken cancellationToken)
    {
        if (contact.Routing.Descriptor.Devices.Count == 0) return false;
        foreach (DeviceRoutingEntry device in contact.Routing.Descriptor.Devices)
        {
            if (!(await _privateGrants.LoadRoutesAsync(
                    contact.UserId,
                    device.Identity.DeviceId,
                    cancellationToken)).Any(route => route.ExpiresAt > DateTimeOffset.UtcNow))
                return false;
        }
        return true;
    }

    private Task<SignedProtocolEvent> SendReceiptAsync(
        LocalContact contact,
        string targetEventId,
        string kind,
        CancellationToken cancellationToken) =>
        SendPayloadAsync(contact, kind, new TargetPayload(2, targetEventId), cancellationToken);

    private async Task<SignedProtocolEvent> SendPayloadAsync<T>(
        LocalContact contact,
        string kind,
        T payload,
        CancellationToken cancellationToken)
    {
        if (contact.PendingApproval)
            throw new InvalidOperationException("Accept the contact request before replying");
        if (!RoutingDescriptorService.Verify(contact.Routing))
        {
            throw new CryptographicException("Contact routing descriptor is invalid");
        }
        byte[] content = JsonSerializer.SerializeToUtf8Bytes(payload, JsonOptions);
        var routesByDevice = new Dictionary<string, List<PublicMailboxRoute>>(StringComparer.Ordinal);
        bool usesPublicContactInbox = false;
        foreach (DeviceRoutingEntry device in contact.Routing.Descriptor.Devices)
        {
            List<PublicMailboxRoute> routes = (await _privateGrants.LoadRoutesAsync(
                    contact.UserId,
                    device.Identity.DeviceId,
                    cancellationToken))
                .Where(route => route.ExpiresAt > DateTimeOffset.UtcNow)
                .ToList();
            if (routes.Count == 0)
            {
                routes = device.Mailboxes
                    .Where(route => route.ExpiresAt > DateTimeOffset.UtcNow)
                    .ToList();
                usesPublicContactInbox |= routes.Count > 0;
            }
            routesByDevice[device.Identity.DeviceId] = routes;
        }
        if (usesPublicContactInbox
            && (kind != "message.text" || payload is not TextPayload text
                || Encoding.UTF8.GetByteCount(text.Text) > MaxContactRequestUtf8Bytes))
        {
            throw new InvalidOperationException(
                "Until a private mailbox capability is received, only a short text contact request can be sent");
        }

        string conversationId = ConversationId(_identity.Current!.UserId, contact.UserId);
        SignedProtocolEvent localEvent = await _runtime.CreateAndQueueEventAsync(
            conversationId,
            kind,
            content,
            cancellationToken);

        // Local persist above is all the UI waits on, so sending feels instant. Encryption and
        // network delivery run in the background, serialized so ratchet state never races across
        // concurrently composed messages.
        _ = DeliverInBackgroundAsync(contact, routesByDevice, localEvent);
        return localEvent;
    }

    private async Task DeliverInBackgroundAsync(
        LocalContact contact,
        Dictionary<string, List<PublicMailboxRoute>> routesByDevice,
        SignedProtocolEvent localEvent)
    {
        await _deliveryGate.WaitAsync();
        try
        {
            IReadOnlyList<OwnedMailboxRoute> ownMailboxes = await _mailboxes.LoadAsync();
            SignedRoutingDescriptor ownRouting = await _ownRouting.LoadAsync();
            IReadOnlyList<PrivateMailboxGrant> replyGrants = ownMailboxes.Select(route => new PrivateMailboxGrant(
                route.Node,
                route.MailboxId,
                route.DeviceHint,
                route.WriteCapability,
                route.ExpiresAt)).ToList();
            var deliveryBody = new DeliveryPackageBody(2, localEvent, ownRouting, replyGrants);
            string deliveryBodyJson = JsonSerializer.Serialize(deliveryBody, JsonOptions);
            var delivery = new SignedDeliveryPackage(
                deliveryBody,
                deliveryBodyJson,
                _identity.SignDeviceData(Encoding.UTF8.GetBytes(deliveryBodyJson)));
            byte[] eventBytes = JsonSerializer.SerializeToUtf8Bytes(delivery, JsonOptions);
            MetadataProtectionMode metadataMode = await _metadataProtection.GetAsync();

            foreach (DeviceRoutingEntry device in contact.Routing.Descriptor.Devices)
            {
                List<PublicMailboxRoute> routes = routesByDevice[device.Identity.DeviceId];
                if (routes.Count == 0) continue;
                string? sessionId = await _ratchet.FindSessionIdAsync(device.Identity.DeviceId);
                V2WireMessage wire;
                if (sessionId is null)
                {
                    ClaimedPrekeyBundle claimed = await _prekeyClient.ClaimAsync(
                        routes[0].BaseUrl,
                        contact.UserId,
                        device.Identity.DeviceId);
                    InitialSessionEnvelope initial = await _ratchet.CreateInitialMessageAsync(claimed, eventBytes);
                    wire = new V2WireMessage(
                        "session.init",
                        localEvent.EventId,
                        _identity.Current,
                        JsonSerializer.Serialize(initial, JsonOptions));
                }
                else
                {
                    RatchetMessage message = await _ratchet.EncryptAsync(sessionId, eventBytes);
                    wire = new V2WireMessage(
                        "ratchet",
                        localEvent.EventId,
                        _identity.Current,
                        JsonSerializer.Serialize(message, JsonOptions));
                }
                byte[] wireBytes = JsonSerializer.SerializeToUtf8Bytes(wire, JsonOptions);
                foreach (PublicMailboxRoute route in routes)
                {
                    MailboxEnvelope envelope = MailboxEnvelopeCodec.Encode(route, wireBytes, TimeSpan.FromDays(7));
                    var job = new DeliveryJobPayload(route, envelope);
                    string jobId = "job1-" + RandomToken(16);
                    await _runtime.Events.QueueDeliveryAsync(
                        jobId,
                        localEvent.EventId,
                        JsonSerializer.SerializeToUtf8Bytes(job, JsonOptions),
                        default,
                        DateTimeOffset.UtcNow + MetadataProtectionSettings.RandomSendDelay(metadataMode));
                }
            }
            await FlushOutboxAsync();
        }
        catch (Exception exception) when (exception is not OperationCanceledException)
        {
            LastFlushError = exception.Message;
        }
        finally
        {
            _deliveryGate.Release();
        }
    }

    public async Task FlushOutboxAsync(CancellationToken cancellationToken = default)
    {
        LastFlushError = null;
        IReadOnlyList<LocalDeliveryJob> jobs = await _runtime.Events.ReadDueDeliveriesAsync(
            cancellationToken: cancellationToken);
        foreach (LocalDeliveryJob job in jobs)
        {
            try
            {
                DeliveryJobPayload payload = JsonSerializer.Deserialize<DeliveryJobPayload>(job.Payload, JsonOptions)
                                             ?? throw new InvalidOperationException("Delivery job is damaged");
                await _transport.PutAsync(payload.Route, payload.Envelope, cancellationToken);
                await _runtime.Events.CompleteDeliveryAsync(job.JobId, job.EventId, cancellationToken);
            }
            catch (Exception exception) when (exception is not OperationCanceledException)
            {
                LastFlushError = exception.Message;
                await _runtime.Events.FailDeliveryAsync(
                    job.JobId,
                    job.AttemptCount,
                    exception.Message,
                    cancellationToken);
            }
        }
    }

    public async Task<int> FetchAsync(CancellationToken cancellationToken = default)
    {
        LastFetchError = null;
        int accepted = 0;
        foreach (OwnedMailboxRoute mailbox in await _mailboxes.LoadAsync(cancellationToken))
        {
            IReadOnlyList<MailboxEnvelope> envelopes;
            try
            {
                envelopes = await _transport.FetchAsync(mailbox, cancellationToken: cancellationToken);
            }
            catch (Exception exception) when (exception is not OperationCanceledException)
            {
                continue;
            }
            foreach (MailboxEnvelope envelope in envelopes)
            {
                try
                {
                    byte[] inner = MailboxEnvelopeCodec.Decode(mailbox, envelope);
                    V2WireMessage wire = JsonSerializer.Deserialize<V2WireMessage>(inner, JsonOptions)
                                         ?? throw new CryptographicException("Wire message is invalid");
                    if (await _runtime.Events.ContainsEventAsync(wire.EventId, cancellationToken))
                    {
                        await _transport.AcknowledgeAsync(mailbox, [envelope.EnvelopeId], cancellationToken);
                        continue;
                    }
                    byte[] eventBytes = wire.Kind switch
                    {
                        "session.init" => (await _ratchet.AcceptInitialMessageAsync(
                                JsonSerializer.Deserialize<InitialSessionEnvelope>(wire.BodyJson, JsonOptions)
                                ?? throw new CryptographicException("Initial wire message is invalid"),
                                cancellationToken))
                            .Plaintext,
                        "ratchet" => (await _ratchet.DecryptAsync(
                                JsonSerializer.Deserialize<RatchetMessage>(wire.BodyJson, JsonOptions)
                                ?? throw new CryptographicException("Ratchet wire message is invalid"),
                                cancellationToken))
                            .Plaintext,
                        _ => throw new CryptographicException("Unsupported wire message kind")
                    };
                    SignedDeliveryPackage delivery = JsonSerializer.Deserialize<SignedDeliveryPackage>(eventBytes, JsonOptions)
                                                     ?? throw new CryptographicException("Signed delivery package is invalid");
                    if (!VerifyDeliveryPackage(delivery, wire.SenderIdentity))
                        throw new CryptographicException("Signed delivery package verification failed");
                    SignedProtocolEvent value = delivery.Body.Event;
                    if (value.EventId != wire.EventId
                        || !ProtocolIdentityService.VerifyEvent(value, wire.SenderIdentity))
                    {
                        throw new CryptographicException("Signed event verification failed");
                    }
                    IReadOnlyList<LocalContact> knownContacts = await _contacts.LoadAsync(cancellationToken);
                    LocalContact? sender = knownContacts.FirstOrDefault(contact => contact.UserId == value.SenderUserId);
                    if ((sender is null || sender.PendingApproval) && !IsValidContactRequest(value))
                    {
                        // The authenticated envelope is deliberately discarded: public contact inboxes
                        // accept only bounded text and must not become attachment or control-event relays.
                        await _transport.AcknowledgeAsync(mailbox, [envelope.EnvelopeId], cancellationToken);
                        continue;
                    }
                    await _privateGrants.ApplyAsync(
                        delivery.Body.SenderRouting,
                        wire.SenderIdentity,
                        delivery.Body.ReplyGrants,
                        cancellationToken);
                    if (sender is null)
                    {
                        sender = new LocalContact(
                            value.SenderUserId,
                            ShortId(value.SenderUserId),
                            delivery.Body.SenderRouting,
                            DateTimeOffset.UtcNow,
                            false,
                            true);
                        await _contacts.SaveAsync(sender, cancellationToken);
                    }
                    else if (delivery.Body.SenderRouting.Descriptor.Sequence > sender.Routing.Descriptor.Sequence)
                    {
                        sender = sender with { Routing = delivery.Body.SenderRouting };
                        await _contacts.SaveAsync(sender, cancellationToken);
                    }
                    bool inserted = await _runtime.AcceptIncomingEventAsync(
                        value,
                        wire.SenderIdentity,
                        envelope.EnvelopeId,
                        envelope.ExpiresAt,
                        cancellationToken);
                    await _transport.AcknowledgeAsync(mailbox, [envelope.EnvelopeId], cancellationToken);
                    if (inserted)
                    {
                        if (value.Kind == "presence.update")
                        {
                            PresencePayload? presence = DeserializePayload<PresencePayload>(value);
                            if (presence is not null)
                            {
                                PresenceReceived?.Invoke(
                                    value.SenderUserId,
                                    DateTimeOffset.FromUnixTimeMilliseconds(presence.AtUnixMs));
                            }
                            // A live signal, not a message the user reads later — don't let it pile up.
                            await _runtime.Events.DeleteEventAsync(value.EventId, cancellationToken);
                        }
                        else
                        {
                            accepted++;
                            if (value.Kind is "message.text" or "message.attachment")
                            {
                                if (!sender.PendingApproval)
                                {
                                    await SendReceiptAsync(
                                        sender,
                                        value.EventId,
                                        "receipt.delivery",
                                        cancellationToken);
                                }
                            }
                        }
                    }
                }
                catch (Exception exception) when (exception is not OperationCanceledException)
                {
                    // Keep the envelope for a later compatible client or repaired local session.
                    LastFetchError = exception.Message;
                }
            }
        }
        return accepted;
    }

    public async Task<IReadOnlyList<LocalTextMessage>> ReadConversationAsync(
        string peerUserId,
        CancellationToken cancellationToken = default)
    {
        string conversationId = ConversationId(_identity.Current!.UserId, peerUserId);
        IReadOnlyList<SignedProtocolEvent> events = await _runtime.Events.ReadConversationAsync(
            conversationId,
            cancellationToken: cancellationToken);
        var projected = new Dictionary<string, ProjectedMessage>(StringComparer.Ordinal);
        foreach (SignedProtocolEvent value in events.Where(value => value.Kind is "message.text" or "message.attachment"))
        {
            string text;
            EncryptedAttachmentManifest? attachment = null;
            if (value.Kind == "message.text")
            {
                TextPayload? payload = DeserializePayload<TextPayload>(value);
                text = payload?.Text ?? "[invalid message]";
            }
            else
            {
                AttachmentPayload? payload = DeserializePayload<AttachmentPayload>(value);
                attachment = payload?.Manifest;
                text = payload is null
                    ? "[invalid attachment]"
                    : string.IsNullOrWhiteSpace(payload.Caption)
                        ? $"📎 {payload.Manifest.FileName} ({payload.Manifest.PlaintextSize} bytes)"
                        : $"{payload.Caption}\n📎 {payload.Manifest.FileName} ({payload.Manifest.PlaintextSize} bytes)";
            }
            projected[value.EventId] = new ProjectedMessage(
                value.EventId,
                value.SenderUserId,
                text,
                DateTimeOffset.FromUnixTimeMilliseconds(value.CreatedAtUnixMilliseconds),
                value.SenderUserId == _identity.Current.UserId,
                attachment);
        }
        foreach (SignedProtocolEvent value in events)
        {
            switch (value.Kind)
            {
                case "message.edit":
                {
                    EditPayload? payload = DeserializePayload<EditPayload>(value);
                    if (payload is not null
                        && projected.TryGetValue(payload.TargetEventId, out ProjectedMessage? message)
                        && message.SenderUserId == value.SenderUserId
                        && message.Attachment is null)
                    {
                        message.Text = payload.Text;
                        message.Edited = true;
                    }
                    break;
                }
                case "message.delete":
                {
                    TargetPayload? payload = DeserializePayload<TargetPayload>(value);
                    if (payload is not null
                        && projected.TryGetValue(payload.TargetEventId, out ProjectedMessage? message)
                        && message.SenderUserId == value.SenderUserId)
                    {
                        message.Text = "Сообщение удалено";
                        message.Deleted = true;
                        message.Attachment = null;
                    }
                    break;
                }
                case "message.reaction":
                {
                    ReactionPayload? payload = DeserializePayload<ReactionPayload>(value);
                    if (payload is not null && payload.Reaction.Length is > 0 and <= 32
                        && projected.TryGetValue(payload.TargetEventId, out ProjectedMessage? message))
                    {
                        string key = value.SenderUserId + "\0" + payload.Reaction;
                        if (payload.Active) message.ReactionState[key] = payload.Reaction;
                        else message.ReactionState.Remove(key);
                    }
                    break;
                }
                case "receipt.delivery":
                case "receipt.read":
                {
                    TargetPayload? payload = DeserializePayload<TargetPayload>(value);
                    if (payload is not null
                        && projected.TryGetValue(payload.TargetEventId, out ProjectedMessage? message)
                        && value.SenderUserId != message.SenderUserId)
                    {
                        message.Delivered = true;
                        if (value.Kind == "receipt.read") message.Read = true;
                    }
                    break;
                }
            }
        }
        return projected.Values
            .Where(value => !value.Deleted)
            .OrderBy(value => value.CreatedAt)
            .ThenBy(value => value.EventId, StringComparer.Ordinal)
            .Select(value => new LocalTextMessage(
                value.EventId,
                value.SenderUserId,
                value.Text,
                value.CreatedAt,
                value.Outgoing,
                value.Edited,
                value.Deleted,
                value.ReactionState.Values.OrderBy(item => item, StringComparer.Ordinal).ToList(),
                value.Delivered,
                value.Read,
                value.Attachment))
            .ToList();
    }

    public static string ConversationId(string leftUserId, string rightUserId)
    {
        string[] users = [leftUserId, rightUserId];
        Array.Sort(users, StringComparer.Ordinal);
        byte[] digest = SHA256.HashData(Encoding.UTF8.GetBytes(users[0] + "\0" + users[1]));
        return "conv1-" + Convert.ToHexString(digest).ToLowerInvariant();
    }

    private static string RandomToken(int bytes) =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
            .TrimEnd('=').Replace('+', '-').Replace('/', '_');

    private static bool VerifyDeliveryPackage(SignedDeliveryPackage delivery, ProtocolIdentity sender)
    {
        try
        {
            return delivery.Body.Version == 2
                   && delivery.Body.Event.SenderUserId == sender.UserId
                   && delivery.Body.Event.SenderDeviceId == sender.DeviceId
                   && delivery.Body.SenderRouting.Descriptor.UserId == sender.UserId
                   && RoutingDescriptorService.Verify(delivery.Body.SenderRouting)
                   && delivery.Body.SenderRouting.Descriptor.Devices.Any(
                       value => value.Identity.DeviceId == sender.DeviceId)
                   && delivery.Body.ReplyGrants.Count <= 20
                   && delivery.Body.ReplyGrants.All(
                       value => PrivateMailboxGrantStore.VerifyGrant(delivery.Body.SenderRouting, sender, value))
                   && delivery.BodyJson == JsonSerializer.Serialize(delivery.Body, JsonOptions)
                   && ProtocolIdentityService.VerifyDeviceData(
                       sender,
                       Encoding.UTF8.GetBytes(delivery.BodyJson),
                       delivery.Signature);
        }
        catch
        {
            return false;
        }
    }

    private static bool IsValidContactRequest(SignedProtocolEvent value)
    {
        if (value.Kind != "message.text") return false;
        TextPayload? payload = DeserializePayload<TextPayload>(value);
        return payload is { Version: 2 }
               && !string.IsNullOrWhiteSpace(payload.Text)
               && Encoding.UTF8.GetByteCount(payload.Text) <= MaxContactRequestUtf8Bytes;
    }

    private static string ShortId(string value) =>
        value.Length <= 18 ? value : value[..10] + "…" + value[^6..];

    private static T? DeserializePayload<T>(SignedProtocolEvent value)
    {
        try
        {
            return JsonSerializer.Deserialize<T>(Convert.FromBase64String(value.Payload), JsonOptions);
        }
        catch
        {
            return default;
        }
    }

    private sealed record TextPayload(int Version, string Text);
    private sealed record EditPayload(int Version, string TargetEventId, string Text);
    private sealed record TargetPayload(int Version, string TargetEventId);
    private sealed record PresencePayload(int Version, long AtUnixMs);
    private sealed record ReactionPayload(int Version, string TargetEventId, string Reaction, bool Active);
    private sealed record AttachmentPayload(int Version, string Caption, EncryptedAttachmentManifest Manifest);
    private sealed record V2WireMessage(
        string Kind,
        string EventId,
        ProtocolIdentity SenderIdentity,
        string BodyJson);
    private sealed record DeliveryPackageBody(
        int Version,
        SignedProtocolEvent Event,
        SignedRoutingDescriptor SenderRouting,
        IReadOnlyList<PrivateMailboxGrant> ReplyGrants);
    private sealed record SignedDeliveryPackage(
        DeliveryPackageBody Body,
        string BodyJson,
        string Signature);
    private sealed record DeliveryJobPayload(PublicMailboxRoute Route, MailboxEnvelope Envelope);

    private sealed class ProjectedMessage
    {
        public ProjectedMessage(
            string eventId,
            string senderUserId,
            string text,
            DateTimeOffset createdAt,
            bool outgoing,
            EncryptedAttachmentManifest? attachment)
        {
            EventId = eventId;
            SenderUserId = senderUserId;
            Text = text;
            CreatedAt = createdAt;
            Outgoing = outgoing;
            Attachment = attachment;
        }

        public string EventId { get; }
        public string SenderUserId { get; }
        public string Text { get; set; }
        public DateTimeOffset CreatedAt { get; }
        public bool Outgoing { get; }
        public bool Edited { get; set; }
        public bool Deleted { get; set; }
        public Dictionary<string, string> ReactionState { get; } = new(StringComparer.Ordinal);
        public bool Delivered { get; set; }
        public bool Read { get; set; }
        public EncryptedAttachmentManifest? Attachment { get; set; }
    }
}
