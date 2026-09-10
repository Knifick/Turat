using System.Buffers.Binary;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using TuratText.Client.LocalFirst;
using TuratText.Client.Services;

namespace TuratText.Client.Crypto.V2;

public sealed record GroupMember(ProtocolIdentity Identity);

public sealed record GroupEpochCommitBody(
    int Version,
    string GroupId,
    long Epoch,
    string ParentCommitHash,
    IReadOnlyList<GroupMember> Members,
    string EpochSecretCommitment,
    ProtocolIdentity Author,
    long CreatedAtUnixMilliseconds);

public sealed record SignedGroupEpochCommit(
    GroupEpochCommitBody Body,
    string BodyJson,
    string CommitHash,
    string Signature);

public sealed record CreatedGroupEpoch(SignedGroupEpochCommit Commit, string EpochSecret);

public sealed record GroupCiphertext(
    int Version,
    string GroupId,
    long Epoch,
    string CommitHash,
    ProtocolIdentity Sender,
    int MessageNumber,
    string Nonce,
    string Ciphertext,
    string Signature);

public enum GroupCommitApplyResult
{
    Applied,
    AlreadyApplied,
    ForkDetected
}

public sealed class GroupEpochService
{
    private const string StoragePurpose = "TuratText.GroupEpochs.v2";
    private const int TagSize = 16;
    private const int MaxMembers = 1024;
    private const int MaxSkippedMessages = 2000;
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);
    private readonly IProtectedStorage _storage;
    private readonly ProtocolIdentityService _identity;
    private readonly SemaphoreSlim _gate = new(1, 1);
    private Dictionary<string, StoredGroup>? _groups;

    public GroupEpochService(IProtectedStorage storage, ProtocolIdentityService identity)
    {
        _storage = storage;
        _identity = identity;
    }

    public async Task<CreatedGroupEpoch> CreateAsync(
        IReadOnlyCollection<GroupMember> members,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        List<GroupMember> normalized = NormalizeMembers(members.Append(new GroupMember(_identity.Current!)));
        string groupId = "grp1-" + RandomToken(18);
        byte[] secret = RandomNumberGenerator.GetBytes(32);
        SignedGroupEpochCommit commit = CreateCommit(groupId, 1, "genesis", normalized, secret);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            _groups![groupId] = BuildState(commit, secret);
            await SaveAsync(cancellationToken);
        }
        finally
        {
            _gate.Release();
        }
        return new CreatedGroupEpoch(commit, Convert.ToBase64String(secret));
    }

    public async Task<CreatedGroupEpoch> CreateMembershipCommitAsync(
        string groupId,
        IReadOnlyCollection<GroupMember> members,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            if (!_groups!.TryGetValue(groupId, out StoredGroup? current))
            {
                throw new InvalidOperationException("Group was not found");
            }
            byte[] secret = RandomNumberGenerator.GetBytes(32);
            SignedGroupEpochCommit commit = CreateCommit(
                groupId,
                checked(current.Epoch + 1),
                current.CommitHash,
                NormalizeMembers(members),
                secret);
            _groups[groupId] = BuildState(commit, secret);
            await SaveAsync(cancellationToken);
            return new CreatedGroupEpoch(commit, Convert.ToBase64String(secret));
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<GroupCommitApplyResult> ApplyCommitAsync(
        SignedGroupEpochCommit commit,
        string epochSecret,
        CancellationToken cancellationToken = default)
    {
        if (!VerifyCommit(commit, Convert.FromBase64String(epochSecret)))
        {
            throw new CryptographicException("Group epoch commit is invalid");
        }
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            if (_groups!.TryGetValue(commit.Body.GroupId, out StoredGroup? current))
            {
                if (commit.Body.Epoch == current.Epoch)
                {
                    return commit.CommitHash == current.CommitHash
                        ? GroupCommitApplyResult.AlreadyApplied
                        : GroupCommitApplyResult.ForkDetected;
                }
                if (commit.Body.Epoch != current.Epoch + 1
                    || commit.Body.ParentCommitHash != current.CommitHash
                    || !current.Members.Any(member => member.Identity.DeviceId == commit.Body.Author.DeviceId))
                {
                    return GroupCommitApplyResult.ForkDetected;
                }
            }
            else if (commit.Body.Epoch != 1 || commit.Body.ParentCommitHash != "genesis")
            {
                return GroupCommitApplyResult.ForkDetected;
            }
            _groups[commit.Body.GroupId] = BuildState(commit, Convert.FromBase64String(epochSecret));
            await SaveAsync(cancellationToken);
            return GroupCommitApplyResult.Applied;
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<GroupCiphertext> EncryptAsync(
        string groupId,
        ReadOnlyMemory<byte> plaintext,
        CancellationToken cancellationToken = default)
    {
        await _identity.GetOrCreateAsync(cancellationToken);
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            StoredGroup group = RequireGroup(groupId);
            StoredSenderChain chain = group.Chains[_identity.Current!.DeviceId];
            byte[] current = Convert.FromBase64String(chain.ChainKey);
            (byte[] messageKey, byte[] next) = AdvanceChain(current);
            var unsigned = new GroupCiphertext(
                2,
                group.GroupId,
                group.Epoch,
                group.CommitHash,
                _identity.Current,
                chain.MessageNumber,
                "",
                "",
                "");
            byte[] nonce = RandomNumberGenerator.GetBytes(12);
            byte[] cipher = new byte[plaintext.Length];
            byte[] tag = new byte[TagSize];
            using (var aes = new AesGcm(messageKey, TagSize))
            {
                aes.Encrypt(nonce, plaintext.Span, cipher, tag, GroupAad(unsigned));
            }
            byte[] packed = [.. cipher, .. tag];
            var encrypted = unsigned with
            {
                Nonce = Convert.ToBase64String(nonce),
                Ciphertext = Convert.ToBase64String(packed)
            };
            encrypted = encrypted with
            {
                Signature = _identity.SignDeviceData(SigningBytes(encrypted))
            };
            chain.ChainKey = Convert.ToBase64String(next);
            chain.MessageNumber++;
            await SaveAsync(cancellationToken);
            Zero(current, messageKey, next);
            return encrypted;
        }
        finally
        {
            _gate.Release();
        }
    }

    public async Task<byte[]> DecryptAsync(
        GroupCiphertext message,
        CancellationToken cancellationToken = default)
    {
        await _gate.WaitAsync(cancellationToken);
        try
        {
            await LoadAsync(cancellationToken);
            StoredGroup group = RequireGroup(message.GroupId);
            if (message.Version != 2 || message.Epoch != group.Epoch || message.CommitHash != group.CommitHash
                || !group.Members.Any(member => member.Identity.DeviceId == message.Sender.DeviceId)
                || !ProtocolIdentityService.VerifyDeviceData(message.Sender, SigningBytes(message), message.Signature))
            {
                throw new CryptographicException("Group message authentication failed");
            }
            StoredSenderChain chain = group.Chains[message.Sender.DeviceId];
            if (message.MessageNumber < 0
                || message.MessageNumber - chain.MessageNumber > MaxSkippedMessages)
            {
                throw new CryptographicException("Group message is replayed or exceeds skip limit");
            }

            byte[] messageKey;
            string nextChainKey = chain.ChainKey;
            int nextMessageNumber = chain.MessageNumber;
            var skipped = new Dictionary<int, string>(chain.SkippedKeys);
            bool consumeSkipped = message.MessageNumber < chain.MessageNumber;
            if (consumeSkipped)
            {
                if (!skipped.TryGetValue(message.MessageNumber, out string? storedKey))
                    throw new CryptographicException("Group message was replayed");
                messageKey = Convert.FromBase64String(storedKey);
            }
            else
            {
                messageKey = [];
                while (nextMessageNumber <= message.MessageNumber)
                {
                    byte[] current = Convert.FromBase64String(nextChainKey);
                    int derivedNumber = nextMessageNumber;
                    (byte[] candidate, byte[] next) = AdvanceChain(current);
                    nextChainKey = Convert.ToBase64String(next);
                    nextMessageNumber++;
                    if (derivedNumber < message.MessageNumber)
                        skipped[derivedNumber] = Convert.ToBase64String(candidate);
                    else
                    {
                        Zero(messageKey);
                        messageKey = candidate;
                        candidate = [];
                    }
                    Zero(current, next, candidate);
                }
            }
            byte[] packed = Convert.FromBase64String(message.Ciphertext);
            if (packed.Length < TagSize) throw new CryptographicException("Group ciphertext is invalid");
            byte[] plaintext = new byte[packed.Length - TagSize];
            try
            {
                using var aes = new AesGcm(messageKey, TagSize);
                aes.Decrypt(
                    Convert.FromBase64String(message.Nonce),
                    packed.AsSpan(0, plaintext.Length),
                    packed.AsSpan(plaintext.Length),
                    plaintext,
                    GroupAad(message));
            }
            catch
            {
                Zero(plaintext, messageKey);
                throw;
            }
            if (consumeSkipped)
                skipped.Remove(message.MessageNumber);
            else
            {
                chain.ChainKey = nextChainKey;
                chain.MessageNumber = nextMessageNumber;
            }
            chain.SkippedKeys = skipped;
            Zero(messageKey);
            await SaveAsync(cancellationToken);
            return plaintext;
        }
        finally
        {
            _gate.Release();
        }
    }

    public static bool VerifyCommit(SignedGroupEpochCommit commit, byte[] epochSecret)
    {
        try
        {
            string json = JsonSerializer.Serialize(commit.Body, JsonOptions);
            string hash = Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(json))).ToLowerInvariant();
            return commit.Body.Version == 2
                   && commit.Body.Epoch > 0
                   && commit.Body.Members.Count is > 0 and <= MaxMembers
                   && commit.Body.Members.Select(member => member.Identity.DeviceId)
                          .Distinct(StringComparer.Ordinal).Count() == commit.Body.Members.Count
                   && commit.Body.Members.All(member =>
                       ProtocolIdentityService.VerifyDeviceCertificate(member.Identity))
                   && commit.BodyJson == json
                   && commit.CommitHash == hash
                   && commit.Body.EpochSecretCommitment == Convert.ToHexString(SHA256.HashData(epochSecret)).ToLowerInvariant()
                   && commit.Body.Members.Any(member => member.Identity.DeviceId == commit.Body.Author.DeviceId)
                   && ProtocolIdentityService.VerifyDeviceData(
                       commit.Body.Author,
                       Encoding.UTF8.GetBytes(json),
                       commit.Signature);
        }
        catch
        {
            return false;
        }
    }

    private SignedGroupEpochCommit CreateCommit(
        string groupId,
        long epoch,
        string parent,
        IReadOnlyList<GroupMember> members,
        byte[] secret)
    {
        if (!members.Any(member => member.Identity.DeviceId == _identity.Current!.DeviceId))
        {
            throw new InvalidOperationException("Commit author must remain a group member");
        }
        var body = new GroupEpochCommitBody(
            2,
            groupId,
            epoch,
            parent,
            members,
            Convert.ToHexString(SHA256.HashData(secret)).ToLowerInvariant(),
            _identity.Current!,
            DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        string json = JsonSerializer.Serialize(body, JsonOptions);
        return new SignedGroupEpochCommit(
            body,
            json,
            Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(json))).ToLowerInvariant(),
            _identity.SignDeviceData(Encoding.UTF8.GetBytes(json)));
    }

    private static StoredGroup BuildState(SignedGroupEpochCommit commit, byte[] secret)
    {
        var chains = new Dictionary<string, StoredSenderChain>(StringComparer.Ordinal);
        foreach (GroupMember member in commit.Body.Members)
        {
            using var hmac = new HMACSHA256(secret);
            byte[] key = hmac.ComputeHash(Encoding.UTF8.GetBytes(
                "TuratText.GroupSenderChain.v2\0" + member.Identity.DeviceId));
            chains[member.Identity.DeviceId] = new StoredSenderChain(Convert.ToBase64String(key), 0);
            Zero(key);
        }
        return new StoredGroup(
            commit.Body.GroupId,
            commit.Body.Epoch,
            commit.CommitHash,
            commit.Body.Members.ToList(),
            chains);
    }

    private static List<GroupMember> NormalizeMembers(IEnumerable<GroupMember> values) =>
        values.DistinctBy(value => value.Identity.DeviceId)
            .OrderBy(value => value.Identity.UserId, StringComparer.Ordinal)
            .ThenBy(value => value.Identity.DeviceId, StringComparer.Ordinal)
            .ToList();

    private StoredGroup RequireGroup(string groupId) =>
        _groups!.TryGetValue(groupId, out StoredGroup? value)
            ? value
            : throw new InvalidOperationException("Group was not found");

    private static (byte[] MessageKey, byte[] Next) AdvanceChain(byte[] chain)
    {
        using var hmac = new HMACSHA256(chain);
        return (hmac.ComputeHash([1]), hmac.ComputeHash([2]));
    }

    private static byte[] SigningBytes(GroupCiphertext value) =>
        [.. GroupAad(value), .. Convert.FromBase64String(value.Nonce), .. Convert.FromBase64String(value.Ciphertext)];

    private static byte[] GroupAad(GroupCiphertext value)
    {
        using var stream = new MemoryStream();
        WriteString(stream, "TuratText.GroupMessage.v2");
        WriteString(stream, value.GroupId);
        Span<byte> number = stackalloc byte[8];
        BinaryPrimitives.WriteInt64BigEndian(number, value.Epoch);
        stream.Write(number);
        WriteString(stream, value.CommitHash);
        WriteString(stream, value.Sender.UserId);
        WriteString(stream, value.Sender.DeviceId);
        Span<byte> messageNumber = stackalloc byte[4];
        BinaryPrimitives.WriteInt32BigEndian(messageNumber, value.MessageNumber);
        stream.Write(messageNumber);
        return stream.ToArray();
    }

    private static void WriteString(Stream stream, string value)
    {
        byte[] bytes = Encoding.UTF8.GetBytes(value);
        Span<byte> length = stackalloc byte[4];
        BinaryPrimitives.WriteInt32BigEndian(length, bytes.Length);
        stream.Write(length);
        stream.Write(bytes);
    }

    private async Task LoadAsync(CancellationToken cancellationToken)
    {
        if (_groups is not null) return;
        if (!File.Exists(StatePath))
        {
            _groups = new Dictionary<string, StoredGroup>(StringComparer.Ordinal);
            return;
        }
        byte[] encrypted = await File.ReadAllBytesAsync(StatePath, cancellationToken);
        _groups = JsonSerializer.Deserialize<Dictionary<string, StoredGroup>>(
                      _storage.Unprotect(encrypted, StoragePurpose),
                      JsonOptions)
                  ?? throw new CryptographicException("Group state is damaged");
    }

    private async Task SaveAsync(CancellationToken cancellationToken)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(StatePath)!);
        byte[] protectedBytes = _storage.Protect(
            JsonSerializer.SerializeToUtf8Bytes(_groups, JsonOptions),
            StoragePurpose);
        string temporary = StatePath + ".new";
        await File.WriteAllBytesAsync(temporary, protectedBytes, cancellationToken);
        File.Move(temporary, StatePath, overwrite: true);
    }

    private string StatePath => Path.Combine(_storage.AppDirectory, "local-first", "group-epochs-v2.secure");

    private static string RandomToken(int bytes) =>
        Convert.ToBase64String(RandomNumberGenerator.GetBytes(bytes))
            .TrimEnd('=').Replace('+', '-').Replace('/', '_');

    private static void Zero(params byte[][] values)
    {
        foreach (byte[] value in values) CryptographicOperations.ZeroMemory(value);
    }

    private sealed record StoredGroup(
        string GroupId,
        long Epoch,
        string CommitHash,
        List<GroupMember> Members,
        Dictionary<string, StoredSenderChain> Chains);

    private sealed class StoredSenderChain
    {
        public StoredSenderChain(string chainKey, int messageNumber)
        {
            ChainKey = chainKey;
            MessageNumber = messageNumber;
        }

        public string ChainKey { get; set; }
        public int MessageNumber { get; set; }
        public Dictionary<int, string> SkippedKeys { get; set; } = [];
    }
}
