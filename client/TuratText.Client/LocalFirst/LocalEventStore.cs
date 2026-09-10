using System.Data;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using Microsoft.Data.Sqlite;
using TuratText.Client.Services;

namespace TuratText.Client.LocalFirst;

public sealed class LocalEventStore : IAsyncDisposable
{
    private const string VaultPurpose = "TuratText.LocalEventVault.v1";
    private const int VaultKeySize = 32;
    private const int NonceSize = 12;
    private const int TagSize = 16;
    private static readonly JsonSerializerOptions JsonOptions = new(JsonSerializerDefaults.Web);

    private readonly IProtectedStorage _storage;
    private readonly SemaphoreSlim _initializationGate = new(1, 1);
    private byte[]? _vaultKey;
    private bool _initialized;

    public LocalEventStore(IProtectedStorage storage)
    {
        _storage = storage;
    }

    public string DatabasePath => Path.Combine(_storage.AppDirectory, "local-first", "events-v2.db");

    public async Task InitializeAsync(CancellationToken cancellationToken = default)
    {
        if (_initialized)
        {
            return;
        }

        await _initializationGate.WaitAsync(cancellationToken);
        try
        {
            if (_initialized)
            {
                return;
            }

            Directory.CreateDirectory(Path.GetDirectoryName(DatabasePath)!);
            _vaultKey = await LoadOrCreateVaultKeyAsync(cancellationToken);
            await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
            await using SqliteCommand command = connection.CreateCommand();
            command.CommandText = """
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS local_schema (
                    version INTEGER NOT NULL
                );
                INSERT INTO local_schema(version)
                    SELECT 1 WHERE NOT EXISTS (SELECT 1 FROM local_schema);

                CREATE TABLE IF NOT EXISTS device_sequences (
                    device_id TEXT PRIMARY KEY,
                    next_sequence INTEGER NOT NULL CHECK(next_sequence > 0)
                );

                CREATE TABLE IF NOT EXISTS events (
                    event_id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL,
                    sender_user_id TEXT NOT NULL,
                    sender_device_id TEXT NOT NULL,
                    device_sequence INTEGER NOT NULL,
                    kind TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    encrypted_event BLOB NOT NULL,
                    nonce BLOB NOT NULL,
                    tag BLOB NOT NULL,
                    delivery_state TEXT NOT NULL,
                    inserted_at_ms INTEGER NOT NULL,
                    UNIQUE(sender_device_id, device_sequence)
                );
                CREATE INDEX IF NOT EXISTS ix_events_conversation_created
                    ON events(conversation_id, created_at_ms, event_id);
                CREATE INDEX IF NOT EXISTS ix_events_outbox
                    ON events(delivery_state, inserted_at_ms);

                CREATE TABLE IF NOT EXISTS processed_envelopes (
                    envelope_id TEXT PRIMARY KEY,
                    processed_at_ms INTEGER NOT NULL,
                    expires_at_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS ix_processed_envelopes_expiry
                    ON processed_envelopes(expires_at_ms);

                CREATE TABLE IF NOT EXISTS delivery_jobs (
                    job_id TEXT PRIMARY KEY,
                    event_id TEXT NOT NULL REFERENCES events(event_id) ON DELETE CASCADE,
                    encrypted_job BLOB NOT NULL,
                    nonce BLOB NOT NULL,
                    tag BLOB NOT NULL,
                    attempt_count INTEGER NOT NULL DEFAULT 0,
                    next_attempt_ms INTEGER NOT NULL,
                    last_error TEXT,
                    created_at_ms INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS ix_delivery_jobs_due
                    ON delivery_jobs(next_attempt_ms, created_at_ms);
                """;
            await command.ExecuteNonQueryAsync(cancellationToken);
            _initialized = true;
        }
        finally
        {
            _initializationGate.Release();
        }
    }

    public async Task<long> ReserveNextDeviceSequenceAsync(
        string deviceId,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        if (string.IsNullOrWhiteSpace(deviceId))
        {
            throw new ArgumentException("Device ID is required", nameof(deviceId));
        }

        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteTransaction transaction = (SqliteTransaction)await connection.BeginTransactionAsync(
            IsolationLevel.Serializable,
            cancellationToken);
        long sequence;
        await using (SqliteCommand select = connection.CreateCommand())
        {
            select.Transaction = transaction;
            select.CommandText = "SELECT next_sequence FROM device_sequences WHERE device_id = $deviceId";
            select.Parameters.AddWithValue("$deviceId", deviceId);
            object? value = await select.ExecuteScalarAsync(cancellationToken);
            sequence = value is null || value is DBNull ? 1 : Convert.ToInt64(value);
        }

        await using (SqliteCommand update = connection.CreateCommand())
        {
            update.Transaction = transaction;
            update.CommandText = """
                INSERT INTO device_sequences(device_id, next_sequence)
                VALUES ($deviceId, $nextSequence)
                ON CONFLICT(device_id) DO UPDATE SET next_sequence = excluded.next_sequence
                """;
            update.Parameters.AddWithValue("$deviceId", deviceId);
            update.Parameters.AddWithValue("$nextSequence", checked(sequence + 1));
            await update.ExecuteNonQueryAsync(cancellationToken);
        }

        await transaction.CommitAsync(cancellationToken);
        return sequence;
    }

    public async Task<bool> AppendAsync(
        SignedProtocolEvent value,
        EventDeliveryState deliveryState,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        EncryptedStoredEvent encrypted = Encrypt(value);

        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = """
            INSERT OR IGNORE INTO events(
                event_id, conversation_id, sender_user_id, sender_device_id,
                device_sequence, kind, created_at_ms, encrypted_event, nonce, tag,
                delivery_state, inserted_at_ms)
            VALUES (
                $eventId, $conversationId, $senderUserId, $senderDeviceId,
                $deviceSequence, $kind, $createdAt, $encryptedEvent, $nonce, $tag,
                $deliveryState, $insertedAt)
            """;
        command.Parameters.AddWithValue("$eventId", value.EventId);
        command.Parameters.AddWithValue("$conversationId", value.ConversationId);
        command.Parameters.AddWithValue("$senderUserId", value.SenderUserId);
        command.Parameters.AddWithValue("$senderDeviceId", value.SenderDeviceId);
        command.Parameters.AddWithValue("$deviceSequence", value.DeviceSequence);
        command.Parameters.AddWithValue("$kind", value.Kind);
        command.Parameters.AddWithValue("$createdAt", value.CreatedAtUnixMilliseconds);
        command.Parameters.AddWithValue("$encryptedEvent", encrypted.Ciphertext);
        command.Parameters.AddWithValue("$nonce", encrypted.Nonce);
        command.Parameters.AddWithValue("$tag", encrypted.Tag);
        command.Parameters.AddWithValue("$deliveryState", deliveryState.ToString());
        command.Parameters.AddWithValue("$insertedAt", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        return await command.ExecuteNonQueryAsync(cancellationToken) == 1;
    }

    public async Task<bool> AppendIncomingAsync(
        SignedProtocolEvent value,
        string envelopeId,
        DateTimeOffset envelopeExpiresAt,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        EncryptedStoredEvent encrypted = Encrypt(value);
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteTransaction transaction = (SqliteTransaction)await connection.BeginTransactionAsync(
            IsolationLevel.Serializable,
            cancellationToken);

        await using (SqliteCommand envelope = connection.CreateCommand())
        {
            envelope.Transaction = transaction;
            envelope.CommandText = """
                INSERT OR IGNORE INTO processed_envelopes(envelope_id, processed_at_ms, expires_at_ms)
                VALUES ($envelopeId, $processedAt, $expiresAt)
                """;
            envelope.Parameters.AddWithValue("$envelopeId", envelopeId);
            envelope.Parameters.AddWithValue("$processedAt", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
            envelope.Parameters.AddWithValue("$expiresAt", envelopeExpiresAt.ToUnixTimeMilliseconds());
            if (await envelope.ExecuteNonQueryAsync(cancellationToken) == 0)
            {
                await transaction.RollbackAsync(cancellationToken);
                return false;
            }
        }

        int inserted;
        await using (SqliteCommand command = connection.CreateCommand())
        {
            command.Transaction = transaction;
            command.CommandText = """
                INSERT OR IGNORE INTO events(
                    event_id, conversation_id, sender_user_id, sender_device_id,
                    device_sequence, kind, created_at_ms, encrypted_event, nonce, tag,
                    delivery_state, inserted_at_ms)
                VALUES (
                    $eventId, $conversationId, $senderUserId, $senderDeviceId,
                    $deviceSequence, $kind, $createdAt, $encryptedEvent, $nonce, $tag,
                    $deliveryState, $insertedAt)
                """;
            command.Parameters.AddWithValue("$eventId", value.EventId);
            command.Parameters.AddWithValue("$conversationId", value.ConversationId);
            command.Parameters.AddWithValue("$senderUserId", value.SenderUserId);
            command.Parameters.AddWithValue("$senderDeviceId", value.SenderDeviceId);
            command.Parameters.AddWithValue("$deviceSequence", value.DeviceSequence);
            command.Parameters.AddWithValue("$kind", value.Kind);
            command.Parameters.AddWithValue("$createdAt", value.CreatedAtUnixMilliseconds);
            command.Parameters.AddWithValue("$encryptedEvent", encrypted.Ciphertext);
            command.Parameters.AddWithValue("$nonce", encrypted.Nonce);
            command.Parameters.AddWithValue("$tag", encrypted.Tag);
            command.Parameters.AddWithValue("$deliveryState", EventDeliveryState.Incoming.ToString());
            command.Parameters.AddWithValue("$insertedAt", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
            inserted = await command.ExecuteNonQueryAsync(cancellationToken);
        }

        await transaction.CommitAsync(cancellationToken);
        return inserted == 1;
    }

    public Task<IReadOnlyList<SignedProtocolEvent>> ReadConversationAsync(
        string conversationId,
        int limit = 500,
        CancellationToken cancellationToken = default) =>
        ReadAsync(
            """
            SELECT event_id, encrypted_event, nonce, tag
            FROM events
            WHERE conversation_id = $value
            ORDER BY created_at_ms ASC, event_id ASC
            LIMIT $limit
            """,
            conversationId,
            limit,
            cancellationToken);

    public async Task DeleteEventAsync(
        string eventId,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = "DELETE FROM events WHERE event_id = $value";
        command.Parameters.AddWithValue("$value", eventId);
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    public async Task DeleteConversationAsync(
        string conversationId,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = "DELETE FROM events WHERE conversation_id = $value";
        command.Parameters.AddWithValue("$value", conversationId);
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    public Task<IReadOnlyList<SignedProtocolEvent>> ReadPendingOutboxAsync(
        int limit = 100,
        CancellationToken cancellationToken = default) =>
        ReadAsync(
            """
            SELECT event_id, encrypted_event, nonce, tag
            FROM events
            WHERE delivery_state = $value
            ORDER BY inserted_at_ms ASC
            LIMIT $limit
            """,
            EventDeliveryState.Pending.ToString(),
            limit,
            cancellationToken);

    public async Task MarkDeliveredAsync(string eventId, CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = """
            UPDATE events SET delivery_state = $state
            WHERE event_id = $eventId AND delivery_state = $pending
            """;
        command.Parameters.AddWithValue("$state", EventDeliveryState.Delivered.ToString());
        command.Parameters.AddWithValue("$pending", EventDeliveryState.Pending.ToString());
        command.Parameters.AddWithValue("$eventId", eventId);
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    public async Task<bool> ContainsEventAsync(string eventId, CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = "SELECT count(*) FROM events WHERE event_id = $eventId";
        command.Parameters.AddWithValue("$eventId", eventId);
        return Convert.ToInt64(await command.ExecuteScalarAsync(cancellationToken)) > 0;
    }

    public async Task CreateDatabaseSnapshotAsync(
        string destinationPath,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        Directory.CreateDirectory(Path.GetDirectoryName(destinationPath)!);
        await using SqliteConnection source = await OpenConnectionAsync(cancellationToken);
        var destinationBuilder = new SqliteConnectionStringBuilder
        {
            DataSource = destinationPath,
            Mode = SqliteOpenMode.ReadWriteCreate,
            Pooling = false
        };
        await using var destination = new SqliteConnection(destinationBuilder.ToString());
        await destination.OpenAsync(cancellationToken);
        source.BackupDatabase(destination);
    }

    public async Task QueueDeliveryAsync(
        string jobId,
        string eventId,
        ReadOnlyMemory<byte> jobPayload,
        CancellationToken cancellationToken = default,
        DateTimeOffset? notBefore = null)
    {
        EnsureInitialized();
        byte[] nonce = RandomNumberGenerator.GetBytes(NonceSize);
        byte[] ciphertext = new byte[jobPayload.Length];
        byte[] tag = new byte[TagSize];
        using (var aes = new AesGcm(_vaultKey!, TagSize))
        {
            aes.Encrypt(nonce, jobPayload.Span, ciphertext, tag, Encoding.UTF8.GetBytes(jobId));
        }
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = """
            INSERT OR IGNORE INTO delivery_jobs(
                job_id, event_id, encrypted_job, nonce, tag, attempt_count,
                next_attempt_ms, created_at_ms)
            VALUES ($jobId, $eventId, $payload, $nonce, $tag, 0, $notBefore, $now)
            """;
        command.Parameters.AddWithValue("$jobId", jobId);
        command.Parameters.AddWithValue("$eventId", eventId);
        command.Parameters.AddWithValue("$payload", ciphertext);
        command.Parameters.AddWithValue("$nonce", nonce);
        command.Parameters.AddWithValue("$tag", tag);
        long now = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
        command.Parameters.AddWithValue("$now", now);
        command.Parameters.AddWithValue("$notBefore", (notBefore ?? DateTimeOffset.UtcNow).ToUnixTimeMilliseconds());
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    public async Task<IReadOnlyList<LocalDeliveryJob>> ReadDueDeliveriesAsync(
        int limit = 100,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        var jobs = new List<LocalDeliveryJob>();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = """
            SELECT job_id, event_id, encrypted_job, nonce, tag, attempt_count
            FROM delivery_jobs
            WHERE next_attempt_ms <= $now
            ORDER BY created_at_ms
            LIMIT $limit
            """;
        command.Parameters.AddWithValue("$now", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        command.Parameters.AddWithValue("$limit", Math.Clamp(limit, 1, 1000));
        await using SqliteDataReader reader = await command.ExecuteReaderAsync(cancellationToken);
        while (await reader.ReadAsync(cancellationToken))
        {
            string jobId = reader.GetString(0);
            byte[] ciphertext = (byte[])reader[2];
            byte[] plaintext = new byte[ciphertext.Length];
            using (var aes = new AesGcm(_vaultKey!, TagSize))
            {
                aes.Decrypt(
                    (byte[])reader[3],
                    ciphertext,
                    (byte[])reader[4],
                    plaintext,
                    Encoding.UTF8.GetBytes(jobId));
            }
            jobs.Add(new LocalDeliveryJob(
                jobId,
                reader.GetString(1),
                plaintext,
                reader.GetInt32(5)));
        }
        return jobs;
    }

    public async Task CompleteDeliveryAsync(
        string jobId,
        string eventId,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteTransaction transaction = (SqliteTransaction)await connection.BeginTransactionAsync(cancellationToken);
        await using (SqliteCommand delete = connection.CreateCommand())
        {
            delete.Transaction = transaction;
            delete.CommandText = "DELETE FROM delivery_jobs WHERE job_id = $jobId";
            delete.Parameters.AddWithValue("$jobId", jobId);
            await delete.ExecuteNonQueryAsync(cancellationToken);
        }
        long remaining;
        await using (SqliteCommand count = connection.CreateCommand())
        {
            count.Transaction = transaction;
            count.CommandText = "SELECT count(*) FROM delivery_jobs WHERE event_id = $eventId";
            count.Parameters.AddWithValue("$eventId", eventId);
            remaining = Convert.ToInt64(await count.ExecuteScalarAsync(cancellationToken));
        }
        if (remaining == 0)
        {
            await using SqliteCommand update = connection.CreateCommand();
            update.Transaction = transaction;
            update.CommandText = "UPDATE events SET delivery_state = $state WHERE event_id = $eventId";
            update.Parameters.AddWithValue("$state", EventDeliveryState.Delivered.ToString());
            update.Parameters.AddWithValue("$eventId", eventId);
            await update.ExecuteNonQueryAsync(cancellationToken);
        }
        await transaction.CommitAsync(cancellationToken);
    }

    public async Task FailDeliveryAsync(
        string jobId,
        int previousAttempts,
        string error,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        int attempt = checked(previousAttempts + 1);
        double delaySeconds = Math.Min(900, Math.Pow(2, Math.Min(attempt, 9)));
        long nextAttempt = DateTimeOffset.UtcNow.AddSeconds(delaySeconds).ToUnixTimeMilliseconds();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = """
            UPDATE delivery_jobs
            SET attempt_count = $attempt, next_attempt_ms = $nextAttempt, last_error = $error
            WHERE job_id = $jobId
            """;
        command.Parameters.AddWithValue("$attempt", attempt);
        command.Parameters.AddWithValue("$nextAttempt", nextAttempt);
        command.Parameters.AddWithValue("$error", error.Length > 500 ? error[..500] : error);
        command.Parameters.AddWithValue("$jobId", jobId);
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    public async Task<bool> TryRecordProcessedEnvelopeAsync(
        string envelopeId,
        DateTimeOffset expiresAt,
        CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = """
            INSERT OR IGNORE INTO processed_envelopes(envelope_id, processed_at_ms, expires_at_ms)
            VALUES ($envelopeId, $processedAt, $expiresAt)
            """;
        command.Parameters.AddWithValue("$envelopeId", envelopeId);
        command.Parameters.AddWithValue("$processedAt", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        command.Parameters.AddWithValue("$expiresAt", expiresAt.ToUnixTimeMilliseconds());
        return await command.ExecuteNonQueryAsync(cancellationToken) == 1;
    }

    public async Task PruneProcessedEnvelopesAsync(CancellationToken cancellationToken = default)
    {
        EnsureInitialized();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = "DELETE FROM processed_envelopes WHERE expires_at_ms < $now";
        command.Parameters.AddWithValue("$now", DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        await command.ExecuteNonQueryAsync(cancellationToken);
    }

    public ValueTask DisposeAsync()
    {
        if (_vaultKey is not null)
        {
            CryptographicOperations.ZeroMemory(_vaultKey);
            _vaultKey = null;
        }
        _initializationGate.Dispose();
        return ValueTask.CompletedTask;
    }

    private async Task<IReadOnlyList<SignedProtocolEvent>> ReadAsync(
        string sql,
        string value,
        int limit,
        CancellationToken cancellationToken)
    {
        EnsureInitialized();
        if (limit is < 1 or > 10_000)
        {
            throw new ArgumentOutOfRangeException(nameof(limit));
        }

        var events = new List<SignedProtocolEvent>();
        await using SqliteConnection connection = await OpenConnectionAsync(cancellationToken);
        await using SqliteCommand command = connection.CreateCommand();
        command.CommandText = sql;
        command.Parameters.AddWithValue("$value", value);
        command.Parameters.AddWithValue("$limit", limit);
        await using SqliteDataReader reader = await command.ExecuteReaderAsync(cancellationToken);
        while (await reader.ReadAsync(cancellationToken))
        {
            string eventId = reader.GetString(0);
            byte[] ciphertext = (byte[])reader[1];
            byte[] nonce = (byte[])reader[2];
            byte[] tag = (byte[])reader[3];
            byte[] plaintext = new byte[ciphertext.Length];
            using (var aes = new AesGcm(_vaultKey!, TagSize))
            {
                aes.Decrypt(nonce, ciphertext, tag, plaintext, Encoding.UTF8.GetBytes(eventId));
            }
            SignedProtocolEvent valueEvent = JsonSerializer.Deserialize<SignedProtocolEvent>(plaintext, JsonOptions)
                                             ?? throw new CryptographicException("Stored event is damaged");
            if (!string.Equals(valueEvent.EventId, eventId, StringComparison.Ordinal))
            {
                throw new CryptographicException("Stored event identifier does not match encrypted data");
            }
            events.Add(valueEvent);
        }
        return events;
    }

    private async Task<byte[]> LoadOrCreateVaultKeyAsync(CancellationToken cancellationToken)
    {
        string path = Path.Combine(_storage.AppDirectory, "local-first", "event-vault-key-v1.secure");
        if (File.Exists(path))
        {
            byte[] protectedBytes = await File.ReadAllBytesAsync(path, cancellationToken);
            byte[] key = _storage.Unprotect(protectedBytes, VaultPurpose);
            if (key.Length != VaultKeySize)
            {
                throw new CryptographicException("Local event vault key has an invalid size");
            }
            return key;
        }

        byte[] created = RandomNumberGenerator.GetBytes(VaultKeySize);
        byte[] encrypted = _storage.Protect(created, VaultPurpose);
        string temporary = path + ".new";
        await File.WriteAllBytesAsync(temporary, encrypted, cancellationToken);
        File.Move(temporary, path, overwrite: true);
        return created;
    }

    private EncryptedStoredEvent Encrypt(SignedProtocolEvent value)
    {
        byte[] plaintext = JsonSerializer.SerializeToUtf8Bytes(value, JsonOptions);
        byte[] nonce = RandomNumberGenerator.GetBytes(NonceSize);
        byte[] ciphertext = new byte[plaintext.Length];
        byte[] tag = new byte[TagSize];
        using var aes = new AesGcm(_vaultKey!, TagSize);
        aes.Encrypt(nonce, plaintext, ciphertext, tag, Encoding.UTF8.GetBytes(value.EventId));
        return new EncryptedStoredEvent(ciphertext, nonce, tag);
    }

    private async Task<SqliteConnection> OpenConnectionAsync(CancellationToken cancellationToken)
    {
        var connection = new SqliteConnection(new SqliteConnectionStringBuilder
        {
            DataSource = DatabasePath,
            Mode = SqliteOpenMode.ReadWriteCreate,
            Cache = SqliteCacheMode.Shared,
            Pooling = false
        }.ToString());
        await connection.OpenAsync(cancellationToken);
        return connection;
    }

    private void EnsureInitialized()
    {
        if (!_initialized || _vaultKey is null)
        {
            throw new InvalidOperationException("Local event store is not initialized");
        }
    }

    private sealed record EncryptedStoredEvent(byte[] Ciphertext, byte[] Nonce, byte[] Tag);
}

public sealed record LocalDeliveryJob(
    string JobId,
    string EventId,
    byte[] Payload,
    int AttemptCount);
