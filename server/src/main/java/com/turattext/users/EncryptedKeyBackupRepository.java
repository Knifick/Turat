package com.turattext.users;

import org.springframework.data.jpa.repository.JpaRepository;

import java.util.UUID;
import java.util.Optional;

public interface EncryptedKeyBackupRepository extends JpaRepository<EncryptedKeyBackup, UUID> {
    Optional<EncryptedKeyBackup> findFirstByUser_IdOrderByCreatedAtDesc(UUID userId);

    void deleteByUser_Id(UUID userId);
}
