package com.turattext.config;

import org.springframework.beans.factory.annotation.Value;
import org.springframework.boot.ApplicationArguments;
import org.springframework.boot.ApplicationRunner;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.stereotype.Component;

@Component
public class H2SchemaCompatibility implements ApplicationRunner {
    private final JdbcTemplate jdbcTemplate;
    private final String datasourceUrl;

    public H2SchemaCompatibility(
            JdbcTemplate jdbcTemplate,
            @Value("${spring.datasource.url:}") String datasourceUrl
    ) {
        this.jdbcTemplate = jdbcTemplate;
        this.datasourceUrl = datasourceUrl;
    }

    @Override
    public void run(ApplicationArguments args) {
        if (datasourceUrl.startsWith("jdbc:h2:")) {
            jdbcTemplate.execute(
                    "ALTER TABLE servers ADD COLUMN IF NOT EXISTS owner_user_id UUID"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE servers ADD COLUMN IF NOT EXISTS replication_key VARCHAR(120)"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE encrypted_key_backups ADD COLUMN IF NOT EXISTS key_id VARCHAR(120)"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE encrypted_key_backups ADD COLUMN IF NOT EXISTS algorithm VARCHAR(80)"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE encrypted_key_backups ADD COLUMN IF NOT EXISTS public_key CHARACTER LARGE OBJECT"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE messages ALTER COLUMN encrypted_content CHARACTER LARGE OBJECT NOT NULL"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE messages ADD COLUMN IF NOT EXISTS reply_to_message_id UUID"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE messages ADD COLUMN IF NOT EXISTS pinned BOOLEAN DEFAULT FALSE NOT NULL"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE messages ADD COLUMN IF NOT EXISTS updated_at TIMESTAMP WITH TIME ZONE"
            );
            jdbcTemplate.execute(
                    "UPDATE messages SET updated_at = server_received_at WHERE updated_at IS NULL"
            );
            jdbcTemplate.execute(
                    "UPDATE messages SET pinned = FALSE WHERE pinned IS NULL"
            );
            jdbcTemplate.execute(
                    "ALTER TABLE encrypted_key_backups ALTER COLUMN encrypted_private_key CHARACTER LARGE OBJECT NOT NULL"
            );
            createV2Schema();
        }
    }

    private void createV2Schema() {
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_mailboxes (
                    id UUID PRIMARY KEY,
                    read_capability_hash VARCHAR(64) NOT NULL UNIQUE,
                    write_capability_hash VARCHAR(64) NOT NULL UNIQUE,
                    contact_capability_hash VARCHAR(64) UNIQUE,
                    device_hint VARCHAR(160) NOT NULL,
                    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    expires_at TIMESTAMP WITH TIME ZONE NOT NULL)
                """);
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_envelopes (
                    id VARCHAR(100) NOT NULL,
                    mailbox_id UUID NOT NULL REFERENCES v2_mailboxes(id) ON DELETE CASCADE,
                    protocol_version INTEGER NOT NULL,
                    recipient_device_hint VARCHAR(160),
                    opaque_payload CHARACTER LARGE OBJECT NOT NULL,
                    size_class INTEGER NOT NULL,
                    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    received_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    access_class VARCHAR(16) DEFAULT 'private' NOT NULL,
                    PRIMARY KEY (mailbox_id, id))
                """);
        jdbcTemplate.execute(
                "ALTER TABLE v2_mailboxes ADD COLUMN IF NOT EXISTS contact_capability_hash VARCHAR(64) UNIQUE");
        jdbcTemplate.execute(
                "ALTER TABLE v2_envelopes ADD COLUMN IF NOT EXISTS access_class VARCHAR(16) DEFAULT 'private' NOT NULL");
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_prekey_bundles (
                    mailbox_id UUID PRIMARY KEY REFERENCES v2_mailboxes(id) ON DELETE CASCADE,
                    user_id VARCHAR(80) NOT NULL,
                    device_id VARCHAR(90) NOT NULL,
                    identity_json CHARACTER LARGE OBJECT NOT NULL,
                    signed_prekey_json CHARACTER LARGE OBJECT NOT NULL,
                    sequence_number BIGINT NOT NULL,
                    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    updated_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    UNIQUE(user_id, device_id))
                """);
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_one_time_prekeys (
                    id UUID PRIMARY KEY,
                    mailbox_id UUID NOT NULL REFERENCES v2_mailboxes(id) ON DELETE CASCADE,
                    prekey_id VARCHAR(100) NOT NULL,
                    prekey_json CHARACTER LARGE OBJECT NOT NULL,
                    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    UNIQUE(mailbox_id, prekey_id))
                """);
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_routing_records (
                    user_id VARCHAR(80) PRIMARY KEY,
                    identity_public_key CHARACTER LARGE OBJECT NOT NULL,
                    sequence_number BIGINT NOT NULL,
                    descriptor_json CHARACTER LARGE OBJECT NOT NULL,
                    signature CHARACTER LARGE OBJECT NOT NULL,
                    device_list_sequence BIGINT NOT NULL DEFAULT 0,
                    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    updated_at TIMESTAMP WITH TIME ZONE NOT NULL)
                """);
        jdbcTemplate.execute(
                "ALTER TABLE v2_routing_records ADD COLUMN IF NOT EXISTS device_list_sequence BIGINT NOT NULL DEFAULT 0");
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_transparency_operations (
                    sequence_number BIGINT GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY,
                    operation_id VARCHAR(100) NOT NULL UNIQUE,
                    operation_type VARCHAR(50) NOT NULL,
                    subject_id VARCHAR(100) NOT NULL,
                    payload_hash VARCHAR(64) NOT NULL,
                    payload CHARACTER LARGE OBJECT NOT NULL,
                    signature CHARACTER LARGE OBJECT NOT NULL,
                    created_at TIMESTAMP WITH TIME ZONE NOT NULL)
                """);
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_blob_objects (
                    id VARCHAR(100) PRIMARY KEY,
                    read_capability_hash VARCHAR(64) NOT NULL UNIQUE,
                    write_capability_hash VARCHAR(64) NOT NULL UNIQUE,
                    expected_size BIGINT NOT NULL,
                    chunk_size INTEGER NOT NULL,
                    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    expires_at TIMESTAMP WITH TIME ZONE NOT NULL)
                """);
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_blob_chunks (
                    object_id VARCHAR(100) NOT NULL REFERENCES v2_blob_objects(id) ON DELETE CASCADE,
                    chunk_index INTEGER NOT NULL,
                    ciphertext CHARACTER LARGE OBJECT NOT NULL,
                    digest VARCHAR(64) NOT NULL,
                    size_bytes INTEGER NOT NULL,
                    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    PRIMARY KEY(object_id, chunk_index))
                """);
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_username_claims (
                    normalized_username VARCHAR(32) NOT NULL,
                    user_id VARCHAR(80) NOT NULL UNIQUE,
                    identity_public_key CHARACTER LARGE OBJECT NOT NULL,
                    sequence_number BIGINT NOT NULL,
                    claim_json CHARACTER LARGE OBJECT NOT NULL,
                    signature CHARACTER LARGE OBJECT NOT NULL,
                    expires_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    updated_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    PRIMARY KEY(normalized_username, user_id))
                """);
        jdbcTemplate.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS ux_v2_username_claims_name ON v2_username_claims(normalized_username)");
        jdbcTemplate.execute("""
                CREATE TABLE IF NOT EXISTS v2_accounts (
                    account_id VARCHAR(40) PRIMARY KEY,
                    login_lookup VARCHAR(64) NOT NULL UNIQUE,
                    recovery_lookup VARCHAR(64) NOT NULL UNIQUE,
                    password_salt VARCHAR(64) NOT NULL,
                    password_verifier VARCHAR(64) NOT NULL,
                    recovery_verifier VARCHAR(64) NOT NULL,
                    access_verifier VARCHAR(64) NOT NULL,
                    password_wrapped_key CHARACTER LARGE OBJECT NOT NULL,
                    recovery_wrapped_key CHARACTER LARGE OBJECT NOT NULL,
                    vault CHARACTER LARGE OBJECT NOT NULL,
                    vault_version BIGINT NOT NULL,
                    snapshot BINARY LARGE OBJECT,
                    snapshot_version BIGINT DEFAULT 0 NOT NULL,
                    snapshot_updated_at TIMESTAMP WITH TIME ZONE,
                    failed_attempts INTEGER DEFAULT 0 NOT NULL,
                    locked_until TIMESTAMP WITH TIME ZONE,
                    recovery_failed_attempts INTEGER DEFAULT 0 NOT NULL,
                    recovery_locked_until TIMESTAMP WITH TIME ZONE,
                    created_at TIMESTAMP WITH TIME ZONE NOT NULL,
                    updated_at TIMESTAMP WITH TIME ZONE NOT NULL)
                """);
    }
}
