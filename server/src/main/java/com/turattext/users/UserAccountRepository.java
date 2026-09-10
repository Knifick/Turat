package com.turattext.users;

import org.springframework.data.jpa.repository.JpaRepository;

import java.util.Optional;
import java.util.UUID;

public interface UserAccountRepository extends JpaRepository<UserAccount, UUID> {
    Optional<UserAccount> findByLoginIgnoreCase(String login);

    boolean existsByLoginIgnoreCase(String login);
}

