alter table servers add column if not exists owner_user_id uuid;
alter table servers add column if not exists replication_key varchar(120);
