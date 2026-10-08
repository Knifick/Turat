package com.turattext.v2;

import jakarta.validation.Valid;
import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.Size;
import org.springframework.http.HttpStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.ResponseStatus;
import org.springframework.web.bind.annotation.RestController;

/** Учётные записи: регистрация, вход по паролю, восстановление по ключу и данные для синхронизации. */
@RestController
@RequestMapping("/v2/accounts")
public class AccountV2Controller {
    private static final String ACCESS = "X-Account-Access";

    private final AccountV2Service accounts;

    public AccountV2Controller(AccountV2Service accounts) {
        this.accounts = accounts;
    }

    @PostMapping
    @ResponseStatus(HttpStatus.CREATED)
    public Created create(@Valid @RequestBody CreateRequest request) {
        return new Created(accounts.create(new AccountV2Service.CreateAccount(
                request.loginLookup(), request.recoveryLookup(), request.passwordSalt(),
                request.passwordVerifier(), request.recoveryVerifier(), request.accessVerifier(),
                request.passwordWrappedKey(), request.recoveryWrappedKey(), request.vault(), request.proofNonce())));
    }

    @PostMapping("/login/salt")
    public Salt salt(@Valid @RequestBody SaltRequest request) {
        return new Salt(accounts.salt(request.loginLookup()));
    }

    @PostMapping("/login")
    public AccountV2Service.AccountSecrets login(@Valid @RequestBody LoginRequest request) {
        return accounts.login(request.loginLookup(), request.passwordAuth());
    }

    @PostMapping("/recover")
    public AccountV2Service.AccountSecrets recover(@Valid @RequestBody RecoverRequest request) {
        return accounts.recover(request.recoveryLookup(), request.recoveryAuth());
    }

    @GetMapping("/{accountId}")
    public AccountV2Service.AccountMeta meta(@PathVariable String accountId, @RequestHeader(ACCESS) String access) {
        return accounts.meta(accountId, access);
    }

    @GetMapping("/{accountId}/vault")
    public AccountV2Service.VaultRecord vault(@PathVariable String accountId, @RequestHeader(ACCESS) String access) {
        return accounts.vault(accountId, access);
    }

    @PutMapping("/{accountId}/vault")
    public Version putVault(
            @PathVariable String accountId,
            @RequestHeader(ACCESS) String access,
            @Valid @RequestBody VaultRequest request
    ) {
        return new Version(accounts.putVault(accountId, access, request.vault(), request.expectedVersion()));
    }

    @PutMapping("/{accountId}/credentials")
    @ResponseStatus(HttpStatus.NO_CONTENT)
    public void putCredentials(
            @PathVariable String accountId,
            @RequestHeader(ACCESS) String access,
            @RequestBody AccountV2Service.Credentials request
    ) {
        accounts.putCredentials(accountId, access, request);
    }

    @GetMapping(value = "/{accountId}/snapshot", produces = MediaType.APPLICATION_OCTET_STREAM_VALUE)
    public ResponseEntity<byte[]> snapshot(@PathVariable String accountId, @RequestHeader(ACCESS) String access) {
        AccountV2Service.SnapshotRecord record = accounts.snapshot(accountId, access);
        return ResponseEntity.ok()
                .contentType(MediaType.APPLICATION_OCTET_STREAM)
                .header("X-Snapshot-Version", Long.toString(record.version()))
                .body(record.data());
    }

    @PutMapping(value = "/{accountId}/snapshot", consumes = MediaType.APPLICATION_OCTET_STREAM_VALUE)
    public Version putSnapshot(
            @PathVariable String accountId,
            @RequestHeader(ACCESS) String access,
            @RequestHeader("X-Snapshot-Base-Version") long baseVersion,
            @RequestBody byte[] data
    ) {
        return new Version(accounts.putSnapshot(accountId, access, data, baseVersion));
    }

    @DeleteMapping("/{accountId}")
    @ResponseStatus(HttpStatus.NO_CONTENT)
    public void delete(@PathVariable String accountId, @RequestHeader(ACCESS) String access) {
        accounts.delete(accountId, access);
    }

    public record CreateRequest(
            @NotBlank String loginLookup,
            @NotBlank String recoveryLookup,
            @NotBlank String passwordSalt,
            @NotBlank String passwordVerifier,
            @NotBlank String recoveryVerifier,
            @NotBlank String accessVerifier,
            @NotBlank String passwordWrappedKey,
            @NotBlank String recoveryWrappedKey,
            @NotBlank @Size(max = 98_304) String vault,
            String proofNonce
    ) {
    }

    public record SaltRequest(@NotBlank String loginLookup) {
    }

    public record LoginRequest(@NotBlank String loginLookup, @NotBlank String passwordAuth) {
    }

    public record RecoverRequest(@NotBlank String recoveryLookup, @NotBlank String recoveryAuth) {
    }

    public record VaultRequest(@NotBlank @Size(max = 98_304) String vault, long expectedVersion) {
    }

    public record Created(String accountId) {
    }

    public record Salt(String passwordSalt) {
    }

    public record Version(long version) {
    }
}
