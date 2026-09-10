alter table v2_envelopes drop constraint v2_envelopes_pkey;
alter table v2_envelopes add primary key (mailbox_id, id);

