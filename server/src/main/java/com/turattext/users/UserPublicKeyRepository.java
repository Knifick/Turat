package com.turattext.users;

import org.springframework.data.jpa.repository.JpaRepository;

import java.util.List;
import java.util.Optional;
import java.util.UUID;

public interface UserPublicKeyRepository extends JpaRepository<UserPublicKey, UUID> {
    Optional<UserPublicKey> findByKeyId(String keyId);

    Optional<UserPublicKey> findFirstByUser_IdAndRevokedAtIsNullOrderByCreatedAtDesc(UUID userId);

    List<UserPublicKey> findAllByUser_IdAndRevokedAtIsNull(UUID userId);
}
