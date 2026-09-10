alter table encrypted_key_backups add column if not exists key_id varchar(120);
alter table encrypted_key_backups add column if not exists algorithm varchar(80);
alter table encrypted_key_backups add column if not exists public_key text;

delete from encrypted_key_backups
where key_id is null or algorithm is null or public_key is null;

alter table encrypted_key_backups alter column key_id set not null;
alter table encrypted_key_backups alter column algorithm set not null;
alter table encrypted_key_backups alter column public_key set not null;
