create table v2_presence_claims (
    user_id varchar(80) not null primary key,
    identity_public_key text not null,
    sequence_number bigint not null,
    claim_json text not null,
    signature text not null,
    expires_at timestamptz not null,
    updated_at timestamptz not null
);

create index ix_v2_presence_claims_expiry
    on v2_presence_claims (expires_at);
