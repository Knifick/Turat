alter table v2_routing_records
    add column device_list_sequence bigint not null default 0;
