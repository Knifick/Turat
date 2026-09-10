package com.turattext.servers;

import org.springframework.data.jpa.repository.JpaRepository;

import java.util.Optional;
import java.util.UUID;

public interface ServerNodeRepository extends JpaRepository<ServerNode, UUID> {
    Optional<ServerNode> findByBaseUrl(String baseUrl);
}

