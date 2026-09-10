package com.turattext.servers;

import org.springframework.data.jpa.repository.JpaRepository;

import java.util.UUID;

public interface ServerSyncStateRepository extends JpaRepository<ServerSyncState, UUID> {
}

