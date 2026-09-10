create table users (
    id uuid primary key,
    login varchar(64) not null,
    password_hash varchar(255) not null,
    enabled boolean not null default true,
    created_at timestamptz not null,
    updated_at timestamptz not null
);

create unique index ux_users_login_lower on users (lower(login));

create table user_profiles (
    user_id uuid primary key references users(id) on delete cascade,
    display_name varchar(100) not null,
    avatar_url varchar(500),
    status varchar(100),
    description varchar(1000),
    updated_at timestamptz not null
);

create index ix_user_profiles_display_name_lower on user_profiles (lower(display_name));

create table user_public_keys (
    id uuid primary key,
    user_id uuid not null references users(id) on delete cascade,
    key_id varchar(120) not null,
    algorithm varchar(80) not null,
    public_key text not null,
    created_at timestamptz not null,
    revoked_at timestamptz
);

create unique index ux_user_public_keys_key_id on user_public_keys (key_id);
create index ix_user_public_keys_user_active on user_public_keys (user_id, revoked_at);

create table chats (
    id uuid primary key,
    type varchar(20) not null,
    created_at timestamptz not null,
    updated_at timestamptz not null
);

create table chat_members (
    chat_id uuid not null references chats(id) on delete cascade,
    user_id uuid not null references users(id) on delete cascade,
    role varchar(30) not null,
    joined_at timestamptz not null,
    primary key (chat_id, user_id)
);

create index ix_chat_members_user on chat_members (user_id);

create table messages (
    id uuid primary key,
    chat_id uuid not null references chats(id) on delete cascade,
    sender_id uuid not null references users(id) on delete cascade,
    encrypted_content text not null,
    nonce varchar(200) not null,
    encryption_key_id varchar(120) not null,
    status varchar(30) not null,
    created_at timestamptz not null,
    server_received_at timestamptz not null
);

create index ix_messages_chat_created on messages (chat_id, created_at);
create index ix_messages_sender on messages (sender_id);

create table servers (
    id uuid primary key,
    name varchar(120) not null,
    base_url varchar(500) not null,
    public_key text,
    role varchar(30) not null,
    status varchar(30) not null,
    last_seen_at timestamptz,
    created_at timestamptz not null,
    updated_at timestamptz not null
);

create unique index ux_servers_base_url on servers (base_url);

create table server_sync_state (
    server_id uuid primary key references servers(id) on delete cascade,
    last_event_id bigint not null default 0,
    last_sync_at timestamptz,
    status varchar(30) not null,
    error_message varchar(1000)
);

create table encrypted_key_backups (
    id uuid primary key,
    user_id uuid not null references users(id) on delete cascade,
    key_id varchar(120) not null,
    algorithm varchar(80) not null,
    public_key text not null,
    salt varchar(200) not null,
    nonce varchar(200) not null,
    kdf varchar(120) not null,
    encrypted_private_key text not null,
    created_at timestamptz not null
);

create index ix_encrypted_key_backups_user on encrypted_key_backups (user_id, created_at desc);
