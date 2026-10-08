-- Учётные записи: зашифрованный «сейф» аккаунта и снимок данных для синхронизации устройств.
--
-- Node не знает ни пароля, ни ключа восстановления, ни того, какому UserID принадлежит запись:
-- логин и ключ восстановления хранятся только как хеши для поиска, пароль — как хеш от ключа,
-- выведенного на устройстве через Argon2id, а сейф и снимок — шифротекст под ключом аккаунта,
-- который Node никогда не видит.
create table v2_accounts (
    account_id varchar(40) primary key,
    login_lookup varchar(64) not null unique,
    recovery_lookup varchar(64) not null unique,
    password_salt varchar(64) not null,
    password_verifier varchar(64) not null,
    recovery_verifier varchar(64) not null,
    access_verifier varchar(64) not null,
    password_wrapped_key text not null,
    recovery_wrapped_key text not null,
    vault text not null,
    vault_version bigint not null,
    snapshot bytea,
    snapshot_version bigint not null default 0,
    snapshot_updated_at timestamptz,
    failed_attempts integer not null default 0,
    locked_until timestamptz,
    recovery_failed_attempts integer not null default 0,
    recovery_locked_until timestamptz,
    created_at timestamptz not null,
    updated_at timestamptz not null
);
