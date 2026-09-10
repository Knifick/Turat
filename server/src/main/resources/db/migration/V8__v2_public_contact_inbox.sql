alter table v2_mailboxes add column contact_capability_hash varchar(64);
create unique index ux_v2_mailboxes_contact_capability
    on v2_mailboxes (contact_capability_hash);

alter table v2_envelopes add column access_class varchar(16) default 'private' not null;
create index ix_v2_envelopes_contact_quota
    on v2_envelopes (mailbox_id, access_class, received_at);
