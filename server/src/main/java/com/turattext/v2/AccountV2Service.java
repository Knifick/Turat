package com.turattext.v2;

import com.turattext.common.BadRequestException;
import com.turattext.common.ConflictException;
import com.turattext.common.NotFoundException;
import com.turattext.common.TooManyRequestsException;
import com.turattext.common.UnauthorizedException;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.dao.DuplicateKeyException;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.time.Duration;
import java.time.Instant;
import java.util.Arrays;
import java.util.Base64;
import java.util.List;
import java.util.regex.Pattern;

import static com.turattext.v2.V2Jdbc.timestamp;

/**
 * Учётные записи Turat.
 *
 * <p>Аккаунт — это «сейф»: личность и ключи пользователя, зашифрованные ключом аккаунта, и снимок
 * его данных для новых устройств. Сам ключ аккаунта лежит здесь дважды, и оба раза завёрнутым:
 * под ключом из пароля (Argon2id на устройстве) и под ключом восстановления. Node проверяет
 * пароль, не зная его: устройство присылает производный «ключ входа», а здесь хранится только его
 * SHA-256. Ни логин, ни ключ восстановления, ни UserID в открытом виде Node не получает — поиск идёт
 * по хешам, поэтому аккаунт не связан ни с телефоном, ни с почтой, ни с чем-то ещё.
 *
 * <p>Перебор пароля онлайн ограничен: после пяти неудач вход закрывается на минуту, затем на два,
 * четыре… до часа. Ключ восстановления считается отдельно — подбор пароля не мешает восстановлению.
 */
@Service
public class AccountV2Service {
    private static final Pattern HEX64 = Pattern.compile("[a-f0-9]{64}");
    private static final Pattern TOKEN = Pattern.compile("[A-Za-z0-9_\\-+/=]{32,128}");
    private static final int MAX_WRAPPED_KEY_CHARS = 256;
    private static final int MAX_VAULT_CHARS = 96 * 1024;
    public static final int MAX_SNAPSHOT_BYTES = 24 * 1024 * 1024;
    private static final int FREE_ATTEMPTS = 5;
    private static final long MAX_LOCK_MINUTES = 60;

    private final JdbcTemplate jdbc;
    private final NodeIdentityService nodeIdentity;
    private final int registrationPowBits;

    public AccountV2Service(
            JdbcTemplate jdbc,
            NodeIdentityService nodeIdentity,
            @Value("${turattext.v2.registration-pow-bits:0}") int registrationPowBits
    ) {
        this.jdbc = jdbc;
        this.nodeIdentity = nodeIdentity;
        this.registrationPowBits = Math.clamp(registrationPowBits, 0, 28);
    }

    @Transactional
    public String create(CreateAccount request) {
        requireHex(request.loginLookup(), "login lookup");
        requireHex(request.recoveryLookup(), "recovery lookup");
        requireHex(request.passwordVerifier(), "password verifier");
        requireHex(request.recoveryVerifier(), "recovery verifier");
        requireHex(request.accessVerifier(), "access verifier");
        requireSalt(request.passwordSalt());
        requireWrapped(request.passwordWrappedKey());
        requireWrapped(request.recoveryWrappedKey());
        requireVault(request.vault());
        requirePow(request.loginLookup(), request.recoveryLookup(), request.proofNonce());
        String accountId = "tta1-" + V2Encoding.sha256Hex(V2Encoding.randomToken(32).getBytes(StandardCharsets.UTF_8))
                .substring(0, 32);
        Instant now = Instant.now();
        try {
            jdbc.update("""
                    insert into v2_accounts(
                        account_id, login_lookup, recovery_lookup, password_salt, password_verifier,
                        recovery_verifier, access_verifier, password_wrapped_key, recovery_wrapped_key,
                        vault, vault_version, created_at, updated_at)
                    values (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)
                    """, accountId, request.loginLookup(), request.recoveryLookup(), request.passwordSalt(),
                    request.passwordVerifier(), request.recoveryVerifier(), request.accessVerifier(),
                    request.passwordWrappedKey(), request.recoveryWrappedKey(), request.vault(),
                    timestamp(now), timestamp(now));
        } catch (DuplicateKeyException exception) {
            throw new ConflictException("This login is already registered on this node");
        }
        return accountId;
    }

    /**
     * Соль пароля для логина. Для несуществующего логина — тоже соль, всегда одна и та же: иначе
     * по ответу можно было бы перебирать, какие логины зарегистрированы.
     */
    public String salt(String loginLookup) {
        requireHex(loginLookup, "login lookup");
        List<String> salts = jdbc.queryForList(
                "select password_salt from v2_accounts where login_lookup = ?", String.class, loginLookup);
        if (!salts.isEmpty()) {
            return salts.getFirst();
        }
        byte[] signature = Base64.getDecoder().decode(
                nodeIdentity.sign(("turat.account.fake-salt\n" + loginLookup).getBytes(StandardCharsets.UTF_8)));
        byte[] digest = sha256(signature);
        return Base64.getEncoder().encodeToString(Arrays.copyOf(digest, 16));
    }

    @Transactional(noRollbackFor = {UnauthorizedException.class, TooManyRequestsException.class})
    public AccountSecrets login(String loginLookup, String passwordAuth) {
        requireHex(loginLookup, "login lookup");
        requireToken(passwordAuth);
        List<StoredAccount> rows = jdbc.query("""
                select account_id, password_verifier as verifier, password_wrapped_key as wrapped,
                       failed_attempts as attempts, locked_until, vault, vault_version, snapshot_version
                from v2_accounts where login_lookup = ? for update
                """, AccountV2Service::mapStored, loginLookup);
        if (rows.isEmpty()) {
            throw new UnauthorizedException("Wrong login or password");
        }
        StoredAccount account = rows.getFirst();
        checkLock(account.lockedUntil());
        if (!verifierMatches(account.verifier(), passwordAuth)) {
            registerFailure(account, "failed_attempts", "locked_until");
            throw new UnauthorizedException("Wrong login or password");
        }
        jdbc.update("update v2_accounts set failed_attempts = 0, locked_until = null where account_id = ?",
                account.accountId());
        return account.secrets();
    }

    @Transactional(noRollbackFor = {UnauthorizedException.class, TooManyRequestsException.class})
    public AccountSecrets recover(String recoveryLookup, String recoveryAuth) {
        requireHex(recoveryLookup, "recovery lookup");
        requireToken(recoveryAuth);
        List<StoredAccount> rows = jdbc.query("""
                select account_id, recovery_verifier as verifier, recovery_wrapped_key as wrapped,
                       recovery_failed_attempts as attempts, recovery_locked_until as locked_until,
                       vault, vault_version, snapshot_version
                from v2_accounts where recovery_lookup = ? for update
                """, AccountV2Service::mapStored, recoveryLookup);
        if (rows.isEmpty()) {
            throw new UnauthorizedException("Recovery key is not valid");
        }
        StoredAccount account = rows.getFirst();
        checkLock(account.lockedUntil());
        if (!verifierMatches(account.verifier(), recoveryAuth)) {
            registerFailure(account, "recovery_failed_attempts", "recovery_locked_until");
            throw new UnauthorizedException("Recovery key is not valid");
        }
        jdbc.update("""
                update v2_accounts set recovery_failed_attempts = 0, recovery_locked_until = null
                where account_id = ?
                """, account.accountId());
        return account.secrets();
    }

    public AccountMeta meta(String accountId, String access) {
        authorize(accountId, access);
        return jdbc.queryForObject("""
                select vault_version, snapshot_version, snapshot_updated_at,
                       coalesce(octet_length(snapshot), 0) as snapshot_bytes
                from v2_accounts where account_id = ?
                """, (row, ignored) -> new AccountMeta(
                row.getLong("vault_version"), row.getLong("snapshot_version"),
                row.getTimestamp("snapshot_updated_at") == null ? null : row.getTimestamp("snapshot_updated_at").toInstant(),
                row.getLong("snapshot_bytes")), accountId);
    }

    public VaultRecord vault(String accountId, String access) {
        authorize(accountId, access);
        return jdbc.queryForObject("select vault, vault_version from v2_accounts where account_id = ?",
                (row, ignored) -> new VaultRecord(row.getString("vault"), row.getLong("vault_version")), accountId);
    }

    @Transactional
    public long putVault(String accountId, String access, String vault, long expectedVersion) {
        authorize(accountId, access);
        requireVault(vault);
        int updated = jdbc.update("""
                update v2_accounts set vault = ?, vault_version = vault_version + 1, updated_at = ?
                where account_id = ? and vault_version = ?
                """, vault, timestamp(Instant.now()), accountId, expectedVersion);
        if (updated == 0) {
            throw new ConflictException("Vault was changed by another device");
        }
        return expectedVersion + 1;
    }

    /** Смена пароля, логина или ключа восстановления. Пустое поле — оставить как есть. */
    @Transactional
    public void putCredentials(String accountId, String access, Credentials request) {
        authorize(accountId, access);
        Instant now = Instant.now();
        try {
            if (request.loginLookup() != null) {
                requireHex(request.loginLookup(), "login lookup");
                jdbc.update("update v2_accounts set login_lookup = ?, updated_at = ? where account_id = ?",
                        request.loginLookup(), timestamp(now), accountId);
            }
            if (request.passwordVerifier() != null) {
                requireHex(request.passwordVerifier(), "password verifier");
                requireSalt(request.passwordSalt());
                requireWrapped(request.passwordWrappedKey());
                jdbc.update("""
                        update v2_accounts set password_salt = ?, password_verifier = ?, password_wrapped_key = ?,
                            failed_attempts = 0, locked_until = null, updated_at = ?
                        where account_id = ?
                        """, request.passwordSalt(), request.passwordVerifier(), request.passwordWrappedKey(),
                        timestamp(now), accountId);
            }
            if (request.recoveryVerifier() != null) {
                requireHex(request.recoveryLookup(), "recovery lookup");
                requireHex(request.recoveryVerifier(), "recovery verifier");
                requireWrapped(request.recoveryWrappedKey());
                jdbc.update("""
                        update v2_accounts set recovery_lookup = ?, recovery_verifier = ?, recovery_wrapped_key = ?,
                            recovery_failed_attempts = 0, recovery_locked_until = null, updated_at = ?
                        where account_id = ?
                        """, request.recoveryLookup(), request.recoveryVerifier(), request.recoveryWrappedKey(),
                        timestamp(now), accountId);
            }
        } catch (DuplicateKeyException exception) {
            throw new ConflictException("This login is already registered on this node");
        }
    }

    public SnapshotRecord snapshot(String accountId, String access) {
        authorize(accountId, access);
        SnapshotRecord record = jdbc.queryForObject(
                "select snapshot, snapshot_version from v2_accounts where account_id = ?",
                (row, ignored) -> new SnapshotRecord(row.getBytes("snapshot"), row.getLong("snapshot_version")),
                accountId);
        if (record == null || record.data() == null) {
            throw new NotFoundException("Snapshot not found");
        }
        return record;
    }

    @Transactional
    public long putSnapshot(String accountId, String access, byte[] data, long baseVersion) {
        authorize(accountId, access);
        if (data == null || data.length < 28 || data.length > MAX_SNAPSHOT_BYTES) {
            throw new BadRequestException("Snapshot size is invalid");
        }
        int updated = jdbc.update("""
                update v2_accounts set snapshot = ?, snapshot_version = snapshot_version + 1,
                    snapshot_updated_at = ?, updated_at = ?
                where account_id = ? and snapshot_version = ?
                """, data, timestamp(Instant.now()), timestamp(Instant.now()), accountId, baseVersion);
        if (updated == 0) {
            throw new ConflictException("Snapshot was changed by another device");
        }
        return baseVersion + 1;
    }

    @Transactional
    public void delete(String accountId, String access) {
        authorize(accountId, access);
        jdbc.update("delete from v2_accounts where account_id = ?", accountId);
    }

    private void authorize(String accountId, String access) {
        if (accountId == null || !accountId.matches("tta1-[a-f0-9]{32}")) {
            throw new BadRequestException("Invalid account id");
        }
        requireToken(access);
        List<String> verifiers = jdbc.queryForList(
                "select access_verifier from v2_accounts where account_id = ?", String.class, accountId);
        if (verifiers.isEmpty() || !verifierMatches(verifiers.getFirst(), access)) {
            throw new UnauthorizedException("Account access denied");
        }
    }

    private void checkLock(Instant lockedUntil) {
        Instant now = Instant.now();
        if (lockedUntil != null && lockedUntil.isAfter(now)) {
            long minutes = Math.max(1, Duration.between(now, lockedUntil).toSeconds() / 60 + 1);
            throw new TooManyRequestsException("Too many attempts. Try again in " + minutes + " min");
        }
    }

    private void registerFailure(StoredAccount account, String attemptsColumn, String lockColumn) {
        int attempts = account.attempts() + 1;
        Instant lockedUntil = null;
        if (attempts >= FREE_ATTEMPTS) {
            long minutes = Math.min(MAX_LOCK_MINUTES, 1L << Math.min(attempts - FREE_ATTEMPTS, 10));
            lockedUntil = Instant.now().plus(Duration.ofMinutes(minutes));
        }
        jdbc.update("update v2_accounts set " + attemptsColumn + " = ?, " + lockColumn + " = ? where account_id = ?",
                attempts, lockedUntil == null ? null : timestamp(lockedUntil), account.accountId());
    }

    private void requirePow(String loginLookup, String recoveryLookup, String nonce) {
        if (registrationPowBits == 0) return;
        if (nonce == null || nonce.length() > 100) {
            throw new BadRequestException("Account proof of work is required");
        }
        byte[] digest = sha256(("turat.account.v1:" + loginLookup + ":" + recoveryLookup + ":" + nonce)
                .getBytes(StandardCharsets.UTF_8));
        int zeros = 0;
        for (byte value : digest) {
            int unsigned = value & 0xff;
            if (unsigned == 0) {
                zeros += 8;
                continue;
            }
            zeros += Integer.numberOfLeadingZeros(unsigned) - 24;
            break;
        }
        if (zeros < registrationPowBits) {
            throw new BadRequestException("Invalid account proof of work");
        }
    }

    private static boolean verifierMatches(String verifier, String token) {
        byte[] expected = verifier.getBytes(StandardCharsets.US_ASCII);
        byte[] actual = V2Encoding.sha256Hex(token.getBytes(StandardCharsets.UTF_8)).getBytes(StandardCharsets.US_ASCII);
        return MessageDigest.isEqual(expected, actual);
    }

    private static byte[] sha256(byte[] value) {
        try {
            return MessageDigest.getInstance("SHA-256").digest(value);
        } catch (java.security.NoSuchAlgorithmException exception) {
            throw new IllegalStateException(exception);
        }
    }

    private static void requireHex(String value, String name) {
        if (value == null || !HEX64.matcher(value).matches()) {
            throw new BadRequestException("Invalid " + name);
        }
    }

    private static void requireToken(String value) {
        if (value == null || !TOKEN.matcher(value).matches()) {
            throw new BadRequestException("Invalid credentials format");
        }
    }

    private static void requireSalt(String value) {
        if (value == null || value.length() < 16 || value.length() > 64) {
            throw new BadRequestException("Invalid password salt");
        }
    }

    private static void requireWrapped(String value) {
        if (value == null || value.length() < 40 || value.length() > MAX_WRAPPED_KEY_CHARS) {
            throw new BadRequestException("Invalid wrapped key");
        }
    }

    private static void requireVault(String value) {
        if (value == null || value.length() < 40 || value.length() > MAX_VAULT_CHARS) {
            throw new BadRequestException("Invalid vault");
        }
    }

    private static StoredAccount mapStored(java.sql.ResultSet row, int ignored) throws java.sql.SQLException {
        return new StoredAccount(
                row.getString("account_id"), row.getString("verifier"), row.getString("wrapped"),
                row.getInt("attempts"),
                row.getTimestamp("locked_until") == null ? null : row.getTimestamp("locked_until").toInstant(),
                row.getString("vault"), row.getLong("vault_version"), row.getLong("snapshot_version"));
    }

    public record CreateAccount(
            String loginLookup,
            String recoveryLookup,
            String passwordSalt,
            String passwordVerifier,
            String recoveryVerifier,
            String accessVerifier,
            String passwordWrappedKey,
            String recoveryWrappedKey,
            String vault,
            String proofNonce
    ) {
    }

    public record Credentials(
            String loginLookup,
            String passwordSalt,
            String passwordVerifier,
            String passwordWrappedKey,
            String recoveryLookup,
            String recoveryVerifier,
            String recoveryWrappedKey
    ) {
    }

    public record AccountSecrets(
            String accountId,
            String wrappedKey,
            String vault,
            long vaultVersion,
            long snapshotVersion
    ) {
    }

    public record AccountMeta(long vaultVersion, long snapshotVersion, Instant snapshotUpdatedAt, long snapshotBytes) {
    }

    public record VaultRecord(String vault, long version) {
    }

    public record SnapshotRecord(byte[] data, long version) {
    }

    private record StoredAccount(
            String accountId,
            String verifier,
            String wrapped,
            int attempts,
            Instant lockedUntil,
            String vault,
            long vaultVersion,
            long snapshotVersion
    ) {
        AccountSecrets secrets() {
            return new AccountSecrets(accountId, wrapped, vault, vaultVersion, snapshotVersion);
        }
    }
}
