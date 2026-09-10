package com.turattext.chats;

import org.springframework.data.jpa.repository.JpaRepository;
import org.springframework.data.jpa.repository.Query;
import org.springframework.data.repository.query.Param;

import java.util.List;
import java.util.Optional;
import java.util.UUID;

public interface ChatMemberRepository extends JpaRepository<ChatMember, ChatMemberId> {
    List<ChatMember> findByUser_Id(UUID userId);

    boolean existsByChat_IdAndUser_Id(UUID chatId, UUID userId);

    List<ChatMember> findByChat_Id(UUID chatId);

    @Query("""
            select c from Chat c
              join c.members m1
              join c.members m2
            where c.type = com.turattext.chats.ChatType.DIRECT
              and m1.user.id = :firstUserId
              and m2.user.id = :secondUserId
            """)
    Optional<Chat> findDirectChat(
            @Param("firstUserId") UUID firstUserId,
            @Param("secondUserId") UUID secondUserId
    );

    @Query("select m.user.id from ChatMember m where m.chat.id = :chatId")
    List<UUID> findMemberIds(@Param("chatId") UUID chatId);
}

