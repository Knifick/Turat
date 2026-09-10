create table v2_username_claims (
    normalized_username varchar(32) not null,
    user_id varchar(80) not null,
    identity_public_key text not null,
    sequence_number bigint not null,
    claim_json text not null,
    signature text not null,
    expires_at timestamptz not null,
    updated_at timestamptz not null,
    primary key (normalized_username, user_id),
    unique (user_id)
);

create index ix_v2_username_claims_expiry
    on v2_username_claims (expires_at);
