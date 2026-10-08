-- Один username — одна учётная запись на Node.
--
-- До этой миграции имя могли занять несколько разных UserID, и поиск по нему отвечал
-- «username связан с несколькими профилями». Дубли сводятся к самой свежей записи — это
-- та учётная запись, которой владелец пользуется сейчас; остальные теряют имя (их профили
-- и переписка не трогаются, а UserID по-прежнему находится напрямую).
delete from v2_username_claims c
where exists (
    select 1 from v2_username_claims newer
    where newer.normalized_username = c.normalized_username
      and (newer.updated_at > c.updated_at
           or (newer.updated_at = c.updated_at and newer.user_id > c.user_id))
);

create unique index ux_v2_username_claims_name on v2_username_claims (normalized_username);
