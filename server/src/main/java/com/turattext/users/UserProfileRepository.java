package com.turattext.users;

import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;
import org.springframework.data.repository.query.Param;

import java.util.List;
import java.util.UUID;

public interface UserProfileRepository extends JpaRepository<UserProfile, UUID> {
    @Query("""
            select p from UserProfile p
            where lower(p.user.login) like lower(concat('%', :query, '%'))
               or lower(p.displayName) like lower(concat('%', :query, '%'))
            order by p.displayName asc
            """)
    List<UserProfile> search(@Param("query") String query);
}

