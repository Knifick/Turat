using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Org.BouncyCastle.Crypto.Kems;
using Org.BouncyCastle.Crypto.Parameters;
using Org.BouncyCastle.Security;
using TuratText.Client.LocalFirst;
using TuratText.Client.Services;

namespace TuratText.Client.Crypto.V2;

public sealed class RatchetSessionService
{
    private const string StoragePurpose = "TuratText.RatchetSessions.v2";
    private const int MaxSkip = 2000;
    private const int NonceSize = 12;
    private const int TagSize = 16;
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly PrekeyStateService _prekeys;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private Dictionary<string, StoredSession>? _sessions;

    public RatchetSessionService(
        IProtectedStorage storage,
        ProtocolIdentityService identity,
        PrekeyStateService prekeys)
    {
        _storage = storage;
        _identity = identity;
        _prekeys = prekeys;
    }

    public async Task<InitialSessionEnvelope> CreateInitialMessageAsync(
        ClaimedPrekeyBundle recipient,
        ReadOnlyMemory<byte> plaintext,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        ValidateClaimedBundle(recipient);
        PrekeyStateService.LocalPrekeySecrets local = await _prekeys.GetSecretsAsync(cancellationToken);
        var senderIdentityDh = PrivateX25519(local.IdentityDhPrivateKey);
        var ephemeral = new X25519PrivateKeyParameters(new SecureRandom());
        DevicePrekeyDescriptor remote = recipient.SignedPrekey.Descriptor;
        byte[] dh1 = Agreement(senderIdentityDh, remote.SignedPrekeyPublicKey);
        byte[] dh2 = Agreement(ephemeral, remote.IdentityDhPublicKey);
        byte[] dh3 = Agreement(ephemeral, remote.SignedPrekeyPublicKey);
        byte[]? dh4 = recipient.OneTimePrekey is null
            ? null
            : Agreement(ephemeral, recipient.OneTimePrekey.PublicKey);

        var pqPublic = MLKemPublicKeyParameters.FromEncoding(
            MLKemParameters.ml_kem_768,
            Convert.FromBase64String(remote.PqPrekeyPublicKey));
        var encapsulator = new MLKemEncapsulator(MLKemParameters.ml_kem_768);
        encapsulator.Init(pqPublic);
        byte[] pqCiphertext = new byte[encapsulator.EncapsulationLength];
        byte[] pqSecret = new byte[encapsulator.SecretLength];
        encapsulator.Encapsulate(pqCiphertext.AsSpan(), pqSecret.AsSpan());

        string sessionId = "ses1-" + RandomToken(18);
        byte[] root = DeriveHandshakeRoot(
            dh1, dh2, dh3, dh4, pqSecret,
            _identity.Current!.UserId,
            recipient.UserId,
            sessionId);
        (byte[] initiatorSend, byte[] initiatorReceive) = InitialChains(root);
        var state = new StoredSession(
            sessionId,
            recipient.UserId,
            recipient.DeviceId,
            Convert.ToBase64String(root),
            Convert.ToBase64String(ephemeral.GetEncoded()),
            remote.SignedPrekeyPublicKey,
            Convert.ToBase64String(initiatorSend),
            Convert.ToBase64String(initiatorReceive),
            0,
            0,
            0,
            false,
            []);

        var header = new InitialHandshakeHeader(
            2,
            sessionId,
            _identity.Current,
            recipient.UserId,
            recipient.DeviceId,
            Convert.ToBase64String(senderIdentityDh.GeneratePublicKey().GetEncoded()),
            Convert.ToBase64String(ephemeral.GeneratePublicKey().GetEncoded()),
            remote.SignedPrekeyId,
            remote.PqPrekeyId,
            recipient.OneTimePrekey?.PrekeyId,
            Convert.ToBase64String(pqCiphertext),
            DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        string headerJson = JsonSerializer.Serialize(header, JsonOptions);

        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            RatchetMessage message = EncryptCore(state, plaintext.Span);
            _sessions![sessionId] = state;
            await SaveAsync(cancellationToken);
            return new InitialSessionEnvelope(
                header,
                headerJson,
                _identity.SignDeviceData(Encoding.UTF8.GetBytes(headerJson)),
                message);
        }
        finally
        {
            _gate.Release();
            Zero(dh1, dh2, dh3, dh4, pqSecret, root, initiatorSend, initiatorReceive);
        }
    }

    public async Task<DecryptedRatchetMessage> AcceptInitialMessageAsync(
        InitialSessionEnvelope envelope,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        InitialHandshakeHeader header = envelope.Header;
        if (header.Version != 2
            || header.RecipientUserId != _identity.Current!.UserId
            || header.RecipientDeviceId != _identity.Current.DeviceId
            || !string.Equals(header.HeaderJson(), envelope.HeaderJson, StringComparison.Ordinal))
        {
            throw new CryptographicException("Initial session header is invalid");
        }
        if (!ProtocolIdentityService.VerifyDeviceData(
                header.SenderIdentity,
                Encoding.UTF8.GetBytes(envelope.HeaderJson),
                envelope.HeaderSignature))
        {
            throw new CryptographicException("Initial session signature is invalid");
        }
        if (Math.Abs(DateTimeOffset.UtcNow.ToUnixTimeMilliseconds() - header.CreatedAtUnixMilliseconds)
            > TimeSpan.FromDays(7).TotalMilliseconds)
        {
            throw new CryptographicException("Initial session header is outside the accepted time window");
        }

        PrekeyStateService.LocalPrekeySecrets local = await _prekeys.GetSecretsAsync(cancellationToken);
        if (header.RecipientSignedPrekeyId != local.SignedPrekeyId
            || header.RecipientPqPrekeyId != local.PqPrekeyId)
        {
            throw new CryptographicException("Initial session targets an unavailable prekey");
        }
        string? oneTimePrivate = await _prekeys.ConsumeOneTimePrivateKeyAsync(
            header.RecipientOneTimePrekeyId,
            cancellationToken);
        if (header.RecipientOneTimePrekeyId is not null && oneTimePrivate is null)
        {
            throw new CryptographicException("One-time prekey has already been consumed");
        }

        var signedPrivate = PrivateX25519(local.SignedPrekeyPrivateKey);
        var identityPrivate = PrivateX25519(local.IdentityDhPrivateKey);
        byte[] dh1 = Agreement(signedPrivate, header.SenderIdentityDhPublicKey);
        byte[] dh2 = Agreement(identityPrivate, header.SenderEphemeralPublicKey);
        byte[] dh3 = Agreement(signedPrivate, header.SenderEphemeralPublicKey);
        byte[]? dh4 = oneTimePrivate is null
            ? null
            : Agreement(PrivateX25519(oneTimePrivate), header.SenderEphemeralPublicKey);
        var pqPrivate = MLKemPrivateKeyParameters.FromEncoding(
            MLKemParameters.ml_kem_768,
            Convert.FromBase64String(local.PqPrekeyPrivateKey));
        var decapsulator = new MLKemDecapsulator(MLKemParameters.ml_kem_768);
        decapsulator.Init(pqPrivate);
        byte[] pqSecret = new byte[decapsulator.SecretLength];
        decapsulator.Decapsulate(Convert.FromBase64String(header.PqCiphertext), pqSecret);
        byte[] root = DeriveHandshakeRoot(
            dh1, dh2, dh3, dh4, pqSecret,
            header.SenderIdentity.UserId,
            _identity.Current.UserId,
            header.SessionId);
        (byte[] initiatorSend, byte[] initiatorReceive) = InitialChains(root);
        var state = new StoredSession(
            header.SessionId,
            header.SenderIdentity.UserId,
            header.SenderIdentity.DeviceId,
            Convert.ToBase64String(root),
            local.SignedPrekeyPrivateKey,
            header.SenderEphemeralPublicKey,
            Convert.ToBase64String(initiatorReceive),
            Convert.ToBase64String(initiatorSend),
            0,
            0,
            0,
            true,
            []);

        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            if (_sessions!.ContainsKey(header.SessionId))
            {
                throw new CryptographicException("Initial session was already processed");
            }
            byte[] plaintext = DecryptCore(state, envelope.Message);
            _sessions[header.SessionId] = state;
            await SaveAsync(cancellationToken);
            return new DecryptedRatchetMessage(
                header.SessionId,
                header.SenderIdentity.UserId,
                header.SenderIdentity.DeviceId,
                plaintext);
        }
        finally
        {
            _gate.Release();
            Zero(dh1, dh2, dh3, dh4, pqSecret, root, initiatorSend, initiatorReceive);
        }
    }

    public async Task<RatchetMessage> EncryptAsync(
        string sessionId,
        ReadOnlyMemory<byte> plaintext,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            if (!_sessions!.TryGetValue(sessionId, out StoredSession? state))
            {
                throw new InvalidOperationException("Ratchet session not found");
            }
            if (state.NeedsSendRatchet) RotateSendingRatchet(state);
            RatchetMessage message = EncryptCore(state, plaintext.Span);
            await SaveAsync(cancellationToken);
            return message;
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<string?> FindSessionIdAsync(
        string peerDeviceId,
        CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            return _sessions!.Values
                .Where(session => session.PeerDeviceId == peerDeviceId)
                .Select(session => session.SessionId)
                .FirstOrDefault();
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<DecryptedRatchetMessage> DecryptAsync(
        RatchetMessage message,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            if (!_sessions!.TryGetValue(message.SessionId, out StoredSession? state))
            {
                throw new InvalidOperationException("Ratchet session not found");
            }
            byte[] plaintext = DecryptCore(state, message);
            await SaveAsync(cancellationToken);
            return new DecryptedRatchetMessage(
                message.SessionId,
                message.SenderUserId,
                message.SenderDeviceId,
                plaintext);
        }
        finally
        {
            _gate.Release();
        }
    }

    private RatchetMessage EncryptCore(StoredSession state, ReadOnlySpan<byte> plaintext)
    {
        byte[] chain = Convert.FromBase64String(state.SendChainKey);
        (byte[] messageKey, byte[] nextChain) = AdvanceChain(chain);
        string publicKey = Convert.ToBase64String(
            PrivateX25519(state.SelfRatchetPrivateKey).GeneratePublicKey().GetEncoded());
        var header = new RatchetMessage(
            2,
            state.SessionId,
            _identity.Current!.UserId,
            _identity.Current.DeviceId,
            state.PeerUserId,
            state.PeerDeviceId,
            publicKey,
            state.PreviousSendingChainLength,
            state.SendingNumber,
            "",
            "");
        byte[] nonce = RandomNumberGenerator.GetBytes(NonceSize);
        byte[] cipher = new byte[plaintext.Length];
        byte[] tag = new byte[TagSize];
        using (var aes = new AesGcm(messageKey, TagSize))
        {
            aes.Encrypt(nonce, plaintext, cipher, tag, RatchetAad(header));
        }
        byte[] packed = new byte[cipher.Length + tag.Length];
        cipher.CopyTo(packed, 0);
        tag.CopyTo(packed, cipher.Length);
        state.SendChainKey = Convert.ToBase64String(nextChain);
        state.SendingNumber++;
        Zero(chain, messageKey, nextChain);
        return header with
        {
            Nonce = Convert.ToBase64String(nonce),
            Ciphertext = Convert.ToBase64String(packed)
        };
    }

    private byte[] DecryptCore(StoredSession state, RatchetMessage message)
    {
        ValidateMessageAddress(state, message);
        string skippedId = SkippedId(message.RatchetPublicKey, message.MessageNumber);
        StoredSkippedKey? skipped = state.SkippedKeys.FirstOrDefault(value => value.Id == skippedId);
        if (skipped is not null)
        {
            state.SkippedKeys.Remove(skipped);
            return DecryptPayload(message, Convert.FromBase64String(skipped.MessageKey));
        }

        if (!string.Equals(message.RatchetPublicKey, state.PeerRatchetPublicKey, StringComparison.Ordinal))
        {
            SkipKeys(state, message.PreviousChainLength);
            RotateReceivingRatchet(state, message.RatchetPublicKey);
        }
        if (message.MessageNumber < state.ReceivingNumber)
        {
            throw new CryptographicException("Ratchet message key is unavailable or was already used");
        }
        if (message.MessageNumber - state.ReceivingNumber > MaxSkip)
        {
            throw new CryptographicException("Ratchet message exceeds the skipped-key limit");
        }
        SkipKeys(state, message.MessageNumber);
        byte[] chain = Convert.FromBase64String(state.ReceiveChainKey);
        (byte[] messageKey, byte[] nextChain) = AdvanceChain(chain);
        state.ReceiveChainKey = Convert.ToBase64String(nextChain);
        state.ReceivingNumber++;
        try
        {
            return DecryptPayload(message, messageKey);
        }
        finally
        {
            Zero(chain, messageKey, nextChain);
        }
    }

    private void RotateSendingRatchet(StoredSession state)
    {
        byte[] root = Convert.FromBase64String(state.RootKey);
        var self = new X25519PrivateKeyParameters(new SecureRandom());
        byte[] dh = Agreement(self, state.PeerRatchetPublicKey);
        (byte[] newRoot, byte[] newChain) = RootKdf(root, dh);
        state.PreviousSendingChainLength = state.SendingNumber;
        state.SendingNumber = 0;
        state.SelfRatchetPrivateKey = Convert.ToBase64String(self.GetEncoded());
        state.RootKey = Convert.ToBase64String(newRoot);
        state.SendChainKey = Convert.ToBase64String(newChain);
        state.NeedsSendRatchet = false;
        Zero(root, dh, newRoot, newChain);
    }

    private void RotateReceivingRatchet(StoredSession state, string newPeerPublicKey)
    {
        byte[] root = Convert.FromBase64String(state.RootKey);
        byte[] receiveDh = Agreement(PrivateX25519(state.SelfRatchetPrivateKey), newPeerPublicKey);
        (byte[] rootAfterReceive, byte[] receiveChain) = RootKdf(root, receiveDh);
        var newSelf = new X25519PrivateKeyParameters(new SecureRandom());
        byte[] sendDh = Agreement(newSelf, newPeerPublicKey);
        (byte[] rootAfterSend, byte[] sendChain) = RootKdf(rootAfterReceive, sendDh);
        state.PreviousSendingChainLength = state.SendingNumber;
        state.SendingNumber = 0;
        state.ReceivingNumber = 0;
        state.PeerRatchetPublicKey = newPeerPublicKey;
        state.SelfRatchetPrivateKey = Convert.ToBase64String(newSelf.GetEncoded());
        state.RootKey = Convert.ToBase64String(rootAfterSend);
        state.ReceiveChainKey = Convert.ToBase64String(receiveChain);
        state.SendChainKey = Convert.ToBase64String(sendChain);
        state.NeedsSendRatchet = false;
        Zero(root, receiveDh, rootAfterReceive, receiveChain, sendDh, rootAfterSend, sendChain);
    }

    private static void SkipKeys(StoredSession state, int until)
    {
        if (until - state.ReceivingNumber > MaxSkip) throw new CryptographicException("Too many skipped messages");
        while (state.ReceivingNumber < until)
        {
            byte[] chain = Convert.FromBase64String(state.ReceiveChainKey);
            (byte[] messageKey, byte[] nextChain) = AdvanceChain(chain);
            state.SkippedKeys.Add(new StoredSkippedKey(
                SkippedId(state.PeerRatchetPublicKey, state.ReceivingNumber),
                Convert.ToBase64String(messageKey)));
            state.ReceiveChainKey = Convert.ToBase64String(nextChain);
            state.ReceivingNumber++;
            Zero(chain, messageKey, nextChain);
        }
        if (state.SkippedKeys.Count > MaxSkip)
        {
            state.SkippedKeys.RemoveRange(0, state.SkippedKeys.Count - MaxSkip);
        }
    }

    private static byte[] DecryptPayload(RatchetMessage message, byte[] messageKey)
    {
        byte[] packed = Convert.FromBase64String(message.Ciphertext);
        if (packed.Length < TagSize) throw new CryptographicException("Ratchet ciphertext is truncated");
        byte[] plaintext = new byte[packed.Length - TagSize];
        using var aes = new AesGcm(messageKey, TagSize);
        aes.Decrypt(
            Convert.FromBase64String(message.Nonce),
            packed.AsSpan(0, plaintext.Length),
            packed.AsSpan(plaintext.Length),
            plaintext,
            RatchetAad(message));
        return plaintext;
    }

    private void ValidateMessageAddress(StoredSession state, RatchetMessage message)
    {
        if (message.Version != 2
            || message.SessionId != state.SessionId
            || message.SenderUserId != state.PeerUserId
            || message.SenderDeviceId != state.PeerDeviceId
            || message.RecipientUserId != _identity.Current!.UserId
            || message.RecipientDeviceId != _identity.Current.DeviceId
            || message.MessageNumber < 0
            || message.PreviousChainLength < 0)
        {
            throw new CryptographicException("Ratchet message address is invalid");
        }
    }

    private static byte[] RatchetAad(RatchetMessage value)
    {
        using var stream = new MemoryStream();
        WriteString(stream, "TuratText.DoubleRatchetMessage.v2");
        WriteInt(stream, value.Version);
        WriteString(stream, value.SessionId);
        WriteString(stream, value.SenderUserId);
        WriteString(stream, value.SenderDeviceId);
        WriteString(stream, value.RecipientUserId);
        WriteString(stream, value.RecipientDeviceId);
        WriteString(stream, value.RatchetPublicKey);
        WriteInt(stream, value.PreviousChainLength);
        WriteInt(stream, value.MessageNumber);
        return stream.ToArray();
    }

    private static (byte[] MessageKey, byte[] NextChain) AdvanceChain(byte[] chain)
    {
        using var hmac = new HMACSHA256(chain);
        return (hmac.ComputeHash([1]), hmac.ComputeHash([2]));
    }

    private static (byte[] Root, byte[] Chain) RootKdf(byte[] root, byte[] dh) =>
        Split(Hkdf(dh, root, Encoding.UTF8.GetBytes("TuratText.RootKdf.v2"), 64));

    private static (byte[] Send, byte[] Receive) InitialChains(byte[] root) =>
        Split(Hkdf(root, new byte[32], Encoding.UTF8.GetBytes("TuratText.InitialChains.v2"), 64));

    private static byte[] DeriveHandshakeRoot(
        byte[] dh1,
        byte[] dh2,
        byte[] dh3,
        byte[]? dh4,
        byte[] pqSecret,
        string senderUserId,
        string recipientUserId,
        string sessionId)
    {
        byte[] input = dh4 is null
            ? [.. dh1, .. dh2, .. dh3, .. pqSecret]
            : [.. dh1, .. dh2, .. dh3, .. dh4, .. pqSecret];
        return Hkdf(
            input,
            new byte[32],
            Encoding.UTF8.GetBytes("TuratText.HybridHandshake.v2\0" + senderUserId + "\0" + recipientUserId + "\0" + sessionId),
            32);
    }

    private static byte[] Hkdf(byte[] ikm, byte[] salt, byte[] info, int length)
    {
        byte[] prk;
        using (var extract = new HMACSHA256(salt)) prk = extract.ComputeHash(ikm);
        byte[] output = new byte[length];
        byte[] previous = [];
        int offset = 0;
        byte counter = 1;
        using var expand = new HMACSHA256(prk);
        while (offset < length)
        {
            byte[] blockInput = [.. previous, .. info, counter++];
            previous = expand.ComputeHash(blockInput);
            int count = Math.Min(previous.Length, length - offset);
            previous.AsSpan(0, count).CopyTo(output.AsSpan(offset));
            offset += count;
        }
        Zero(prk, previous);
        return output;
    }

    private static (byte[] Left, byte[] Right) Split(byte[] value)
    {
        byte[] left = value[..32];
        byte[] right = value[32..64];
        Zero(value);
        return (left, right);
    }

    private static byte[] Agreement(X25519PrivateKeyParameters privateKey, string publicKey)
    {
        byte[] secret = new byte[X25519PrivateKeyParameters.SecretSize];
        privateKey.GenerateSecret(
            new X25519PublicKeyParameters(Convert.FromBase64String(publicKey)),
            secret,
            0);
        return secret;
    }

    private static X25519PrivateKeyParameters PrivateX25519(string value) =>
        new(Convert.FromBase64String(value));

    private static void ValidateClaimedBundle(ClaimedPrekeyBundle value)
    {
        var publication = new PrekeyPublication(
            value.Identity,
            value.SignedPrekey,
            value.OneTimePrekey is null ? [] : [value.OneTimePrekey]);
        if (!PrekeyStateService.VerifyPublication(publication)
            || value.UserId != value.Identity.UserId
            || value.DeviceId != value.Identity.DeviceId
            || value.SignedPrekey.Descriptor.ExpiresAtUnixMilliseconds <= DateTimeOffset.UtcNow.ToUnixTimeMilliseconds())
        {
            throw new CryptographicException("Claimed prekey bundle is invalid");
        }
    }

    private async Task LoadAsync(CancellationToken cancellationToken)
    {
        if (_sessions is not null) return;
        if (!File.Exists(StatePath))
        {
            _sessions = new Dictionary<string, StoredSession>(StringComparer.Ordinal);
            return;
        }
        byte[] encrypted = await File.ReadAllBytesAsync(StatePath, cancellationToken);
        _sessions = JsonSerializer.Deserialize<Dictionary<string, StoredSession>>(
                        _storage.Unprotect(encrypted, StoragePurpose),
                        JsonOptions)
                    ?? throw new CryptographicException("Ratchet session store is damaged");
    }

    private async Task SaveAsync(CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(StatePath)!);
        byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(_sessions, JsonOptions);
        byte[] encrypted = _storage.Protect(plaintext, StoragePurpose);
        string temporary = StatePath + ".new";
        await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
        File.Move(temporary, StatePath, overwrite: true);
    }

    private string StatePath => Path.Combine(_storage.AppDirectory, "local-first", "ratchet-sessions-v2.secure");

    private static string SkippedId(string ratchetPublicKey, int number) => ratchetPublicKey + ":" + number;

    private static string RandomToken(int bytes) =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
            .TrimEnd('=').Replace('+', '-').Replace('/', '_');

    private static void WriteString(Stream stream, string value)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(value);
        WriteInt(stream, bytes.Length);
        stream.Write(bytes);
    }

    private static void WriteInt(Stream stream, int value)
    {
        Span<byte> bytes = stackalloc byte[4];
        BinaryPrimitives.WriteInt32BigEndian(bytes, value);
        stream.Write(bytes);
    }

    private static void Zero(params byte[]?[] values)
    {
        foreach (byte[]? value in values)
        {
            if (value is not null) CryptographicOperations.ZeroMemory(value);
        }
    }

    private sealed class StoredSession
    {
        public StoredSession(
            string sessionId,
            string peerUserId,
            string peerDeviceId,
            string rootKey,
            string selfRatchetPrivateKey,
            string peerRatchetPublicKey,
            string sendChainKey,
            string receiveChainKey,
            int sendingNumber,
            int receivingNumber,
            int previousSendingChainLength,
            bool needsSendRatchet,
            List<StoredSkippedKey> skippedKeys)
        {
            SessionId = sessionId;
            PeerUserId = peerUserId;
            PeerDeviceId = peerDeviceId;
            RootKey = rootKey;
            SelfRatchetPrivateKey = selfRatchetPrivateKey;
            PeerRatchetPublicKey = peerRatchetPublicKey;
            SendChainKey = sendChainKey;
            ReceiveChainKey = receiveChainKey;
            SendingNumber = sendingNumber;
            ReceivingNumber = receivingNumber;
            PreviousSendingChainLength = previousSendingChainLength;
            NeedsSendRatchet = needsSendRatchet;
            SkippedKeys = skippedKeys;
        }

        public string SessionId { get; set; }
        public string PeerUserId { get; set; }
        public string PeerDeviceId { get; set; }
        public string RootKey { get; set; }
        public string SelfRatchetPrivateKey { get; set; }
        public string PeerRatchetPublicKey { get; set; }
        public string SendChainKey { get; set; }
        public string ReceiveChainKey { get; set; }
        public int SendingNumber { get; set; }
        public int ReceivingNumber { get; set; }
        public int PreviousSendingChainLength { get; set; }
        public bool NeedsSendRatchet { get; set; }
        public List<StoredSkippedKey> SkippedKeys { get; set; }
    }

    private sealed record StoredSkippedKey(string Id, string MessageKey);
}

internal static class InitialHandshakeHeaderExtensions
{
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    public static string HeaderJson(this InitialHandshakeHeader value) =>
        JsonSerializer.Serialize(value, JsonOptions);
}
