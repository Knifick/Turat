using System.Text;
using System.Text.Json;
using TuratText.Client.Crypto.V2;
using TuratText.Client.LocalFirst;
using TuratText.Client.Messaging.V2;
using TuratText.Client.Services;
using TuratText.Client.Transport.V2;
using TuratText.Client.Updates;

string temporaryDirectory = Path.Combine(
    Path.GetTempPath(),
    "TuratText-LocalFirstSmoke-" + Guid.NewGuid().ToString("N"));
Directory.CreateDirectory(temporaryDirectory);

try
{
    var storage = new TestProtectedStorage(temporaryDirectory);
    ProtocolIdentity firstIdentity;
    SignedProtocolEvent firstEvent;

    await using (var runtime = new LocalFirstRuntime(
                     new ProtocolIdentityService(storage),
                     new LocalEventStore(storage)))
    {
        await runtime.InitializeAsync();
        firstIdentity = runtime.Identity;
        Require(ProtocolIdentityService.VerifyDeviceCertificate(firstIdentity), "device certificate");

        firstEvent = await runtime.CreateAndQueueEventAsync(
            "conversation-smoke",
            "message.created",
            Encoding.UTF8.GetBytes("opaque-e2ee-payload"));
        Require(firstEvent.DeviceSequence == 1, "first device sequence");
        Require(ProtocolIdentityService.VerifyEvent(firstEvent, firstIdentity), "event signature");

        SignedProtocolEvent tampered = firstEvent with { Kind = "message.deleted" };
        Require(!ProtocolIdentityService.VerifyEvent(tampered, firstIdentity), "tamper rejection");

        IReadOnlyList<SignedProtocolEvent> pending = await runtime.Events.ReadPendingOutboxAsync();
        Require(pending.Count == 1 && pending[0].EventId == firstEvent.EventId, "pending outbox");
        Require(!await runtime.Events.AppendAsync(firstEvent, EventDeliveryState.Pending), "event deduplication");

        DateTimeOffset expiry = DateTimeOffset.UtcNow.AddDays(1);
        Require(await runtime.Events.TryRecordProcessedEnvelopeAsync("envelope-smoke", expiry), "first envelope");
        Require(!await runtime.Events.TryRecordProcessedEnvelopeAsync("envelope-smoke", expiry), "envelope deduplication");

        var senderStorage = new TestProtectedStorage(Path.Combine(temporaryDirectory, "sender"));
        await using (var sender = new LocalFirstRuntime(
                         new ProtocolIdentityService(senderStorage),
                         new LocalEventStore(senderStorage)))
        {
            await sender.InitializeAsync();
            SignedProtocolEvent incoming = await sender.CreateAndQueueEventAsync(
                "incoming-conversation",
                "message.created",
                Encoding.UTF8.GetBytes("incoming-opaque-payload"));
            Require(
                await runtime.AcceptIncomingEventAsync(incoming, sender.Identity, "incoming-envelope", expiry),
                "accepted incoming event");
            Require(
                !await runtime.AcceptIncomingEventAsync(incoming, sender.Identity, "incoming-envelope", expiry),
                "duplicate incoming envelope");
            Require(
                !await runtime.AcceptIncomingEventAsync(incoming, sender.Identity, "second-envelope-copy", expiry),
                "duplicate incoming event");
        }

        byte[] databaseBytes = await File.ReadAllBytesAsync(runtime.Events.DatabasePath);
        Require(
            databaseBytes.AsSpan().IndexOf(Encoding.UTF8.GetBytes("opaque-e2ee-payload")) < 0,
            "encrypted event at rest");

        await runtime.Events.MarkDeliveredAsync(firstEvent.EventId);
        Require((await runtime.Events.ReadPendingOutboxAsync()).Count == 0, "delivered outbox state");
    }

    await using (var restarted = new LocalFirstRuntime(
                     new ProtocolIdentityService(storage),
                     new LocalEventStore(storage)))
    {
        await restarted.InitializeAsync();
        Require(restarted.Identity.UserId == firstIdentity.UserId, "persistent UserID");
        Require(restarted.Identity.DeviceId == firstIdentity.DeviceId, "persistent DeviceID");
        SignedProtocolEvent secondEvent = await restarted.CreateAndQueueEventAsync(
            "conversation-smoke",
            "message.created",
            Encoding.UTF8.GetBytes("second-opaque-payload"));
        Require(secondEvent.DeviceSequence == 2, "persistent device sequence");

        IReadOnlyList<SignedProtocolEvent> history = await restarted.Events.ReadConversationAsync("conversation-smoke");
        Require(history.Count == 2, "local encrypted history");
    }

    await VerifyDeviceLinkAsync(temporaryDirectory);
    await VerifyEncryptedBackupAsync(temporaryDirectory);
    await VerifyHybridRatchetAsync(temporaryDirectory);
    await VerifyGroupEpochsAsync(temporaryDirectory);
    await VerifySignedReleaseAsync();
    string? smokeNode = Environment.GetEnvironmentVariable("TURATTEXT_SMOKE_NODE");
    if (Uri.TryCreate(smokeNode, UriKind.Absolute, out Uri? smokeNodeUri))
    {
        await VerifyNodeRoundTripAsync(temporaryDirectory, smokeNodeUri);
        await VerifyUsernameDirectoryAsync(temporaryDirectory, smokeNodeUri);
        await VerifyDiscoveryBundleAsync(temporaryDirectory, smokeNodeUri);
        await VerifyEncryptedAttachmentAsync(temporaryDirectory, smokeNodeUri);
        await VerifyMessagingRoundTripAsync(temporaryDirectory, smokeNodeUri);
    }

    Console.WriteLine("Local-first smoke tests passed.");
}
finally
{
    Directory.Delete(temporaryDirectory, recursive: true);
}

static async Task VerifyDeviceLinkAsync(string root)
{
    var sourceStorage = new TestProtectedStorage(Path.Combine(root, "link-source"));
    using var sourceIdentity = new ProtocolIdentityService(sourceStorage);
    ProtocolIdentity source = await sourceIdentity.GetOrCreateAsync();
    var sourceDevices = new DeviceListService(sourceStorage, sourceIdentity);
    var sourceLink = new DeviceLinkService(sourceStorage, sourceIdentity, sourceDevices);
    byte[] package = await sourceLink.CreatePackageAsync("device link secret");
    Require(package.AsSpan().IndexOf(Encoding.UTF8.GetBytes(source.UserId)) < 0, "encrypted device link package");

    var targetStorage = new TestProtectedStorage(Path.Combine(root, "link-target"));
    using var targetIdentity = new ProtocolIdentityService(targetStorage);
    var targetDevices = new DeviceListService(targetStorage, targetIdentity);
    var targetLink = new DeviceLinkService(targetStorage, targetIdentity, targetDevices);
    DeviceLinkResult linked = await targetLink.ImportPackageAsync(package, "device link secret", false);
    Require(linked.Identity.UserId == source.UserId, "linked device UserID");
    Require(linked.Identity.DeviceId != source.DeviceId, "independent linked DeviceID");
    Require(!targetIdentity.HasIdentityAuthority, "linked device does not receive identity master key");
    Require(linked.DeviceList.Document.Devices.Count == 2 && DeviceListService.Verify(linked.DeviceList),
        "signed multi-device list");
}

static async Task VerifyEncryptedBackupAsync(string root)
{
    var sourceStorage = new TestProtectedStorage(Path.Combine(root, "backup-source"));
    ProtocolIdentity sourceIdentity;
    string backupPath = Path.Combine(root, "local-first.ttbackup");
    await using (var source = new LocalFirstRuntime(
                     new ProtocolIdentityService(sourceStorage),
                     new LocalEventStore(sourceStorage)))
    {
        await source.InitializeAsync();
        sourceIdentity = source.Identity;
        await source.CreateAndQueueEventAsync(
            "backup-conversation",
            "message.text",
            Encoding.UTF8.GetBytes("backup secret"));
        var backup = new LocalFirstBackupService(sourceStorage, source.Events);
        await backup.CreateAsync(backupPath, "correct horse battery staple");
    }
    byte[] container = await File.ReadAllBytesAsync(backupPath);
    Require(container.AsSpan().IndexOf(Encoding.UTF8.GetBytes("backup secret")) < 0, "encrypted backup container");

    var restoredStorage = new TestProtectedStorage(Path.Combine(root, "backup-restored"));
    var unusedEvents = new LocalEventStore(restoredStorage);
    var restore = new LocalFirstBackupService(restoredStorage, unusedEvents);
    await restore.RestoreAsync(backupPath, "correct horse battery staple");
    await using var restored = new LocalFirstRuntime(
        new ProtocolIdentityService(restoredStorage),
        new LocalEventStore(restoredStorage));
    await restored.InitializeAsync();
    Require(restored.Identity.UserId == sourceIdentity.UserId, "backup identity restore");
    IReadOnlyList<SignedProtocolEvent> history = await restored.Events.ReadConversationAsync("backup-conversation");
    Require(history.Count == 1, "backup local event restore");
}

static async Task VerifyGroupEpochsAsync(string root)
{
    var aliceStorage = new TestProtectedStorage(Path.Combine(root, "group-alice"));
    var bobStorage = new TestProtectedStorage(Path.Combine(root, "group-bob"));
    using var aliceIdentity = new ProtocolIdentityService(aliceStorage);
    using var bobIdentity = new ProtocolIdentityService(bobStorage);
    ProtocolIdentity alice = await aliceIdentity.GetOrCreateAsync();
    ProtocolIdentity bob = await bobIdentity.GetOrCreateAsync();
    var aliceGroups = new GroupEpochService(aliceStorage, aliceIdentity);
    var bobGroups = new GroupEpochService(bobStorage, bobIdentity);

    CreatedGroupEpoch created = await aliceGroups.CreateAsync([new GroupMember(bob)]);
    Require(GroupEpochService.VerifyCommit(created.Commit, Convert.FromBase64String(created.EpochSecret)),
        "signed group genesis commit");
    Require(await bobGroups.ApplyCommitAsync(created.Commit, created.EpochSecret) == GroupCommitApplyResult.Applied,
        "group genesis apply");

    GroupCiphertext ciphertext = await aliceGroups.EncryptAsync(
        created.Commit.Body.GroupId,
        Encoding.UTF8.GetBytes("group hello"));
    byte[] plaintext = await bobGroups.DecryptAsync(ciphertext);
    Require(Encoding.UTF8.GetString(plaintext) == "group hello", "group epoch encryption");

    GroupCiphertext groupFirst = await aliceGroups.EncryptAsync(
        created.Commit.Body.GroupId,
        Encoding.UTF8.GetBytes("group out of order one"));
    GroupCiphertext groupSecond = await aliceGroups.EncryptAsync(
        created.Commit.Body.GroupId,
        Encoding.UTF8.GetBytes("group out of order two"));
    byte[] groupSecondPlaintext = await bobGroups.DecryptAsync(groupSecond);
    byte[] groupFirstPlaintext = await bobGroups.DecryptAsync(groupFirst);
    Require(Encoding.UTF8.GetString(groupSecondPlaintext) == "group out of order two",
        "group skipped-key advance");
    Require(Encoding.UTF8.GetString(groupFirstPlaintext) == "group out of order one",
        "group skipped-key recovery");

    CreatedGroupEpoch next = await aliceGroups.CreateMembershipCommitAsync(
        created.Commit.Body.GroupId,
        [new GroupMember(alice), new GroupMember(bob)]);
    CreatedGroupEpoch competing = await bobGroups.CreateMembershipCommitAsync(
        created.Commit.Body.GroupId,
        [new GroupMember(alice), new GroupMember(bob)]);
    var observerStorage = new TestProtectedStorage(Path.Combine(root, "group-observer"));
    using var observerIdentity = new ProtocolIdentityService(observerStorage);
    await observerIdentity.GetOrCreateAsync();
    var observerGroups = new GroupEpochService(observerStorage, observerIdentity);
    Require(await observerGroups.ApplyCommitAsync(created.Commit, created.EpochSecret) == GroupCommitApplyResult.Applied,
        "group observer genesis");
    Require(await observerGroups.ApplyCommitAsync(next.Commit, next.EpochSecret) == GroupCommitApplyResult.Applied,
        "group epoch transition");
    Require(await observerGroups.ApplyCommitAsync(competing.Commit, competing.EpochSecret) == GroupCommitApplyResult.ForkDetected,
        "group fork detection");
}

static async Task VerifySignedReleaseAsync()
{
    string releaseDirectory = Path.Combine(
        Environment.CurrentDirectory,
        "server", "deploy", "v2", "updates", "stable");
    string manifestPath = Path.Combine(releaseDirectory, "manifest.json");
    if (!File.Exists(manifestPath)) return;
    SignedUpdateManifest manifest = JsonSerializer.Deserialize<SignedUpdateManifest>(
                                        await File.ReadAllTextAsync(manifestPath),
                                        new JsonSerializerOptions(JsonSerializerDefaults.Web))
                                    ?? throw new InvalidOperationException("Smoke release manifest is invalid");
    VerifiedUpdate verified = ThresholdUpdateVerifier.Verify(manifest, ReleaseTrustRoot.Current);
    foreach (UpdateArtifact artifact in verified.Manifest.Body.Artifacts)
    {
        byte[] content = await File.ReadAllBytesAsync(Path.Combine(
            releaseDirectory,
            Path.GetFileName(artifact.Url)));
        ThresholdUpdateVerifier.VerifyArtifact(artifact, content);
    }
    Require(verified.Manifest.Body.Sequence == 2, "2-of-3 signed release manifest");
}

static async Task VerifyHybridRatchetAsync(string root)
{
    var aliceStorage = new TestProtectedStorage(Path.Combine(root, "ratchet-alice"));
    var bobStorage = new TestProtectedStorage(Path.Combine(root, "ratchet-bob"));
    using var aliceIdentity = new ProtocolIdentityService(aliceStorage);
    using var bobIdentity = new ProtocolIdentityService(bobStorage);
    await aliceIdentity.GetOrCreateAsync();
    await bobIdentity.GetOrCreateAsync();
    var alicePrekeys = new PrekeyStateService(aliceStorage, aliceIdentity);
    var bobPrekeys = new PrekeyStateService(bobStorage, bobIdentity);
    var aliceRatchet = new RatchetSessionService(aliceStorage, aliceIdentity, alicePrekeys);
    var bobRatchet = new RatchetSessionService(bobStorage, bobIdentity, bobPrekeys);

    PrekeyPublication bobPublication = await bobPrekeys.GetOrCreatePublicationAsync(4);
    Require(PrekeyStateService.VerifyPublication(bobPublication), "signed hybrid prekeys");
    var claimed = new ClaimedPrekeyBundle(
        bobPublication.Identity.UserId,
        bobPublication.Identity.DeviceId,
        bobPublication.Identity,
        bobPublication.SignedPrekey,
        bobPublication.OneTimePrekeys[0],
        bobPublication.SignedPrekey.Descriptor.Sequence,
        DateTimeOffset.FromUnixTimeMilliseconds(bobPublication.SignedPrekey.Descriptor.ExpiresAtUnixMilliseconds));

    InitialSessionEnvelope initial = await aliceRatchet.CreateInitialMessageAsync(
        claimed,
        Encoding.UTF8.GetBytes("hybrid hello"));
    DecryptedRatchetMessage initialPlaintext = await bobRatchet.AcceptInitialMessageAsync(initial);
    Require(Encoding.UTF8.GetString(initialPlaintext.Plaintext) == "hybrid hello", "PQ hybrid initial message");

    RatchetMessage reply = await bobRatchet.EncryptAsync(
        initial.Header.SessionId,
        Encoding.UTF8.GetBytes("ratcheted reply"));
    DecryptedRatchetMessage replyPlaintext = await aliceRatchet.DecryptAsync(reply);
    Require(Encoding.UTF8.GetString(replyPlaintext.Plaintext) == "ratcheted reply", "DH ratchet reply");

    RatchetMessage first = await aliceRatchet.EncryptAsync(
        initial.Header.SessionId,
        Encoding.UTF8.GetBytes("out of order one"));
    RatchetMessage second = await aliceRatchet.EncryptAsync(
        initial.Header.SessionId,
        Encoding.UTF8.GetBytes("out of order two"));
    DecryptedRatchetMessage secondPlaintext = await bobRatchet.DecryptAsync(second);
    DecryptedRatchetMessage firstPlaintext = await bobRatchet.DecryptAsync(first);
    Require(Encoding.UTF8.GetString(secondPlaintext.Plaintext) == "out of order two", "skipped message advance");
    Require(Encoding.UTF8.GetString(firstPlaintext.Plaintext) == "out of order one", "skipped message recovery");

    RatchetMessage tampered = second with { Ciphertext = Convert.ToBase64String(new byte[32]) };
    bool rejected = false;
    try
    {
        await bobRatchet.DecryptAsync(tampered);
    }
    catch (System.Security.Cryptography.CryptographicException)
    {
        rejected = true;
    }
    Require(rejected, "ratchet tamper rejection");
}

static async Task VerifyNodeRoundTripAsync(string root, Uri nodeUri)
{
    var storage = new TestProtectedStorage(Path.Combine(root, "node-roundtrip"));
    using var identity = new ProtocolIdentityService(storage);
    await identity.GetOrCreateAsync();
    var prekeys = new PrekeyStateService(storage, identity);
    using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
    using var transport = new HttpsMailboxTransport(http);
    NodeDescriptor node = await transport.GetNodeDescriptorAsync(nodeUri);
    Require(NodeDescriptorVerifier.Verify(node), "node descriptor interop");
    OwnedMailboxRoute owned = await transport.RegisterMailboxAsync(node, identity.Current!.DeviceId);

    var prekeyClient = new PrekeyNodeClient(http);
    PrekeyPublication publication = await prekeys.GetOrCreatePublicationAsync(4);
    await prekeyClient.PublishAsync(owned, publication);
    ClaimedPrekeyBundle claim = await prekeyClient.ClaimAsync(
        node.BaseUrl,
        identity.Current.UserId,
        identity.Current.DeviceId);
    Require(claim.OneTimePrekey is not null, "node one-time prekey claim");

    var devices = new DeviceListService(storage, identity);
    var routing = new RoutingDescriptorService(storage, identity, prekeys, devices);
    SignedRoutingDescriptor signedRoute = await routing.CreateAsync([owned]);
    var routingClient = new RoutingNodeClient(http);
    await routingClient.PublishAsync(node, signedRoute);
    SignedRoutingDescriptor downloaded = await routingClient.GetAsync(node, identity.Current.UserId);
    Require(downloaded.Descriptor.Sequence == signedRoute.Descriptor.Sequence, "routing publication roundtrip");

    PublicMailboxRoute publicRoute = signedRoute.Descriptor.Devices[0].Mailboxes[0];
    byte[] inner = Encoding.UTF8.GetBytes("signed inner E2EE envelope");
    MailboxEnvelope envelope = MailboxEnvelopeCodec.Encode(publicRoute, inner, TimeSpan.FromHours(1));
    await transport.PutAsync(publicRoute, envelope);
    IReadOnlyList<MailboxEnvelope> fetched = await transport.FetchAsync(owned);
    MailboxEnvelope received = fetched.Single(value => value.EnvelopeId == envelope.EnvelopeId);
    Require(MailboxEnvelopeCodec.Decode(owned, received).SequenceEqual(inner), "mailbox randomized outer envelope");
    await transport.AcknowledgeAsync(owned, [received.EnvelopeId]);
    Require((await transport.FetchAsync(owned)).Count == 0, "mailbox acknowledgement");
}

static async Task VerifyEncryptedAttachmentAsync(string root, Uri nodeUri)
{
    using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
    using var transport = new HttpsMailboxTransport(http);
    NodeDescriptor node = await transport.GetNodeDescriptorAsync(nodeUri);
    var attachments = new EncryptedAttachmentService(http);
    byte[] original = Encoding.UTF8.GetBytes(string.Concat(Enumerable.Repeat(
        "encrypted attachment smoke payload\n",
        40_000)));
    EncryptedAttachmentManifest manifest = await attachments.EncryptAndUploadAsync(
        original,
        "smoke.txt",
        "text/plain",
        [node]);
    Require(manifest.FileKey.Length > 32 && manifest.ChunkCount > 1, "attachment E2EE manifest");
    byte[] downloaded = await attachments.DownloadAndDecryptAsync(manifest);
    Require(downloaded.SequenceEqual(original), "chunked attachment roundtrip");
}

static async Task VerifyUsernameDirectoryAsync(string root, Uri nodeUri)
{
    var storage = new TestProtectedStorage(Path.Combine(root, "username-directory"));
    using var identity = new ProtocolIdentityService(storage);
    await identity.GetOrCreateAsync();
    using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
    using var transport = new HttpsMailboxTransport(http);
    NodeDescriptor node = await transport.GetNodeDescriptorAsync(nodeUri);
    var usernames = new UsernameDirectoryClient(http, storage, identity);
    string username = "smoke." + identity.Current!.UserId.Substring(4, 10);
    SignedUsernameClaim published = await usernames.PublishAsync([node], username);
    Require(UsernameDirectoryClient.Verify(published), "signed username claim");
    IReadOnlyList<SignedUsernameClaim> resolved = await usernames.ResolveAsync([node], "@" + username);
    Require(resolved.Count == 1 && resolved[0].Claim.UserId == identity.Current!.UserId,
        "username directory roundtrip");
}

static async Task VerifyDiscoveryBundleAsync(string root, Uri nodeUri)
{
    var sourceStorage = new TestProtectedStorage(Path.Combine(root, "discovery-source"));
    using var identity = new ProtocolIdentityService(sourceStorage);
    await identity.GetOrCreateAsync();
    using var http = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
    using var transport = new HttpsMailboxTransport(http);
    NodeDescriptor node = await transport.GetNodeDescriptorAsync(nodeUri);
    OwnedMailboxRoute mailbox = await transport.RegisterMailboxAsync(node, identity.Current!.DeviceId);
    var mailboxes = new OwnedMailboxStore(sourceStorage);
    await mailboxes.SaveAsync([mailbox]);
    var source = new DiscoveryBundleService(
        identity,
        mailboxes,
        new BootstrapNodeSource(sourceStorage),
        new RelayDescriptorSource(sourceStorage));
    byte[] bundle = await source.ExportAsync();

    var targetStorage = new TestProtectedStorage(Path.Combine(root, "discovery-target"));
    using var targetIdentity = new ProtocolIdentityService(targetStorage);
    var targetBootstrap = new BootstrapNodeSource(targetStorage);
    var target = new DiscoveryBundleService(
        targetIdentity,
        new OwnedMailboxStore(targetStorage),
        targetBootstrap,
        new RelayDescriptorSource(targetStorage));
    (int nodes, int relays) = await target.ImportAsync(bundle);
    Require(nodes == 1 && relays == 0, "social bridge bundle import");
    Require((await targetBootstrap.LoadAsync()).Any(value => value.ExpectedNodeId == node.NodeId),
        "social bridge node pinning");
}

static async Task VerifyMessagingRoundTripAsync(string root, Uri nodeUri)
{
    await using var alice = await V2TestClient.CreateAsync(Path.Combine(root, "messaging-alice"), nodeUri);
    await using var bob = await V2TestClient.CreateAsync(Path.Combine(root, "messaging-bob"), nodeUri);

    SignedRoutingDescriptor bobRouting = await alice.RoutingClient.GetAsync(
        alice.Node,
        bob.Identity.Current!.UserId);
    var bobContact = new LocalContact(
        bob.Identity.Current.UserId,
        "Bob",
        bobRouting,
        DateTimeOffset.UtcNow,
        false);
    await alice.Contacts.SaveAsync(bobContact);

    SignedRoutingDescriptor aliceRouting = await bob.RoutingClient.GetAsync(
        bob.Node,
        alice.Identity.Current!.UserId);
    var aliceContact = new LocalContact(
        alice.Identity.Current.UserId,
        "Alice",
        aliceRouting,
        DateTimeOffset.UtcNow,
        false);

    SignedProtocolEvent hello = await alice.Messaging.SendTextAsync(bobContact, "network hello");
    Require(await bob.Messaging.FetchAsync() == 1,
        "mailbox receive initial session"
        + (alice.Messaging.LastFlushError is null ? "" : ": send=" + alice.Messaging.LastFlushError)
        + (bob.Messaging.LastFetchError is null ? "" : ": receive=" + bob.Messaging.LastFetchError));
    IReadOnlyList<LocalTextMessage> bobHistory = await bob.Messaging.ReadConversationAsync(alice.Identity.Current.UserId);
    Require(bobHistory.Count == 1 && bobHistory[0].Text == "network hello", "local-first received history");
    LocalContact pending = (await bob.Contacts.LoadAsync()).Single(value => value.UserId == alice.Identity.Current.UserId);
    Require(pending.PendingApproval, "unknown sender becomes contact request");
    Require(await alice.Messaging.FetchAsync() == 0, "pending contact does not leak delivery receipt");
    await bob.Contacts.SetPendingApprovalAsync(alice.Identity.Current.UserId, false);
    aliceContact = (await bob.Contacts.LoadAsync()).Single(value => value.UserId == alice.Identity.Current.UserId);

    await bob.Messaging.SendTextAsync(aliceContact, "network reply");
    Require(await alice.Messaging.FetchAsync() == 1, "mailbox receive ratchet reply");
    IReadOnlyList<LocalTextMessage> aliceHistory = await alice.Messaging.ReadConversationAsync(bob.Identity.Current.UserId);
    Require(aliceHistory.Count == 2 && aliceHistory[^1].Text == "network reply", "bidirectional local-first history");
    Require(await bob.Messaging.FetchAsync() == 1, "reply delivery receipt");
    bobHistory = await bob.Messaging.ReadConversationAsync(alice.Identity.Current.UserId);
    Require(bobHistory[^1].Delivered, "private capability delivery receipt");

    await alice.Messaging.EditMessageAsync(bobContact, hello.EventId, "network hello edited");
    Require(await bob.Messaging.FetchAsync() == 1, "message edit event");
    bobHistory = await bob.Messaging.ReadConversationAsync(alice.Identity.Current.UserId);
    Require(bobHistory[0].Edited && bobHistory[0].Text == "network hello edited", "message edit projection");

    await bob.Messaging.SetReactionAsync(aliceContact, hello.EventId, "👍", true);
    Require(await alice.Messaging.FetchAsync() == 1, "reaction event");
    aliceHistory = await alice.Messaging.ReadConversationAsync(bob.Identity.Current.UserId);
    Require(aliceHistory[0].Reactions.Contains("👍"), "reaction projection");

    await alice.Messaging.DeleteMessageAsync(bobContact, hello.EventId);
    Require(await bob.Messaging.FetchAsync() == 1, "message tombstone event");
    bobHistory = await bob.Messaging.ReadConversationAsync(alice.Identity.Current.UserId);
    Require(bobHistory[0].Deleted, "message tombstone projection");

    var aliceDeviceLists = new DeviceListService(alice.Storage, alice.Identity);
    var aliceDeviceLink = new DeviceLinkService(alice.Storage, alice.Identity, aliceDeviceLists);
    byte[] linkedPackage = await aliceDeviceLink.CreatePackageAsync("network linked device secret");
    await alice.Provisioning.ProvisionAsync([(nodeUri, null)]);
    await using var linkedAlice = await V2TestClient.CreateAsync(
        Path.Combine(root, "network-alice-linked"),
        nodeUri,
        linkedPackage,
        "network linked device secret");
    SignedRoutingDescriptor linkedRouting = await bob.RoutingClient.GetAsync(bob.Node, alice.Identity.Current.UserId);
    Require(!linkedAlice.Identity.HasIdentityAuthority, "network linked device has no master identity key");
    Require(linkedRouting.Descriptor.Devices.Count == 2, "linked device routing fanout");

    await aliceDeviceLists.RevokeAsync(linkedAlice.Identity.Current!.DeviceId, "smoke lost device");
    await alice.Provisioning.ProvisionAsync([(nodeUri, null)]);
    bool revokedPublisherRejected = false;
    try
    {
        await linkedAlice.Provisioning.ProvisionAsync([(nodeUri, null)]);
    }
    catch (System.Security.Cryptography.CryptographicException)
    {
        revokedPublisherRejected = true;
    }
    Require(revokedPublisherRejected, "revoked linked device cannot republish routing");
    SignedRoutingDescriptor revokedRouting = await bob.RoutingClient.GetAsync(bob.Node, alice.Identity.Current.UserId);
    Require(revokedRouting.Descriptor.Devices.Count == 1, "revoked device removed from routing fanout");
}

static void Require(bool condition, string name)
{
    if (!condition)
    {
        throw new InvalidOperationException($"Smoke test failed: {name}");
    }
}

file sealed class TestProtectedStorage(string appDirectory) : IProtectedStorage
{
    public string AppDirectory { get; } = appDirectory;

    public byte[] Protect(byte[] plaintext, string purpose) =>
        Transform(plaintext, purpose);

    public byte[] Unprotect(byte[] ciphertext, string purpose) =>
        Transform(ciphertext, purpose);

    private static byte[] Transform(byte[] value, string purpose)
    {
        byte[] purposeBytes = System.Security.Cryptography.SHA256.HashData(Encoding.UTF8.GetBytes(purpose));
        byte[] result = new byte[value.Length];
        for (int index = 0; index < value.Length; index++)
        {
            result[index] = (byte)(value[index] ^ purposeBytes[index % purposeBytes.Length]);
        }
        return result;
    }
}

file sealed class V2TestClient : IAsyncDisposable
{
    private V2TestClient(
        ProtocolIdentityService identity,
        LocalFirstRuntime runtime,
        HttpClient http,
        HttpsMailboxTransport transport,
        LocalContactStore contacts,
        RoutingNodeClient routingClient,
        NodeDescriptor node,
        V2MessagingService messaging,
        TestProtectedStorage storage,
        MailboxProvisioningService provisioning)
    {
        Identity = identity;
        Runtime = runtime;
        Http = http;
        Transport = transport;
        Contacts = contacts;
        RoutingClient = routingClient;
        Node = node;
        Messaging = messaging;
        Storage = storage;
        Provisioning = provisioning;
    }

    public ProtocolIdentityService Identity { get; }
    public LocalFirstRuntime Runtime { get; }
    public HttpClient Http { get; }
    public HttpsMailboxTransport Transport { get; }
    public LocalContactStore Contacts { get; }
    public RoutingNodeClient RoutingClient { get; }
    public NodeDescriptor Node { get; }
    public V2MessagingService Messaging { get; }
    public TestProtectedStorage Storage { get; }
    public MailboxProvisioningService Provisioning { get; }

    public static async Task<V2TestClient> CreateAsync(
        string directory,
        Uri nodeUri,
        byte[]? linkPackage = null,
        string? linkPassphrase = null)
    {
        var storage = new TestProtectedStorage(directory);
        var identity = new ProtocolIdentityService(storage);
        var events = new LocalEventStore(storage);
        var runtime = new LocalFirstRuntime(identity, events);
        var devices = new DeviceListService(storage, identity);
        if (linkPackage is not null)
        {
            var link = new DeviceLinkService(storage, identity, devices);
            await link.ImportPackageAsync(
                linkPackage,
                linkPassphrase ?? throw new ArgumentNullException(nameof(linkPassphrase)),
                false);
        }
        await runtime.InitializeAsync();
        var prekeys = new PrekeyStateService(storage, identity);
        var ratchet = new RatchetSessionService(storage, identity, prekeys);
        var http = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
        var transport = new HttpsMailboxTransport(http);
        var mailboxes = new OwnedMailboxStore(storage);
        var routing = new RoutingDescriptorService(storage, identity, prekeys, devices);
        var prekeyClient = new PrekeyNodeClient(http);
        var routingClient = new RoutingNodeClient(http);
        var transparency = new TransparencyLogClient(http, storage);
        var provisioning = new MailboxProvisioningService(
            transport,
            mailboxes,
            identity,
            prekeys,
            routing,
            prekeyClient,
            routingClient,
            transparency,
            new OwnRoutingDescriptorStore(storage));
        IReadOnlyList<OwnedMailboxRoute> routes = await provisioning.ProvisionAsync([(nodeUri, null)]);
        IReadOnlyList<OwnedMailboxRoute> reprovisioned = await provisioning.ProvisionAsync([(nodeUri, null)]);
        if (reprovisioned[0].MailboxId != routes[0].MailboxId)
            throw new InvalidOperationException("Smoke test failed: idempotent mailbox reprovisioning");
        var contacts = new LocalContactStore(storage);
        var attachments = new EncryptedAttachmentService(http);
        var messaging = new V2MessagingService(
            runtime,
            identity,
            ratchet,
            mailboxes,
            contacts,
            transport,
            prekeyClient,
            attachments,
            new OwnRoutingDescriptorStore(storage),
            new PrivateMailboxGrantStore(storage),
            new MetadataProtectionSettings(storage));
        return new V2TestClient(
            identity, runtime, http, transport, contacts, routingClient, routes[0].Node, messaging,
            storage, provisioning);
    }

    public async ValueTask DisposeAsync()
    {
        await Runtime.DisposeAsync();
        Transport.Dispose();
        Http.Dispose();
    }
}
