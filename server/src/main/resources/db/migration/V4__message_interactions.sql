alter table messages add column reply_to_message_id uuid;
alter table messages add column pinned boolean not null default false;
alter table messages add column updated_at timestamptz;
update messages set updated_at = server_received_at where updated_at is null;
alter table messages alter column updated_at set not null;

create table message_reactions (
    message_id uuid not null references messages(id) on delete cascade,
    user_id uuid not null references users(id) on delete cascade,
    reaction varchar(24) not null,
    active boolean not null default true,
    updated_at timestamptz not null,
    primary key (message_id, user_id)
);

create index ix_message_reactions_message_active
    on message_reactions (message_id, active);
