package com.turattext.chats;

import com.turattext.chats.dto.ChatResponse;
import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import com.turattext.users.UserAccount;
import com.turattext.users.UserAccountRepository;
import com.turattext.users.UserProfile;
import com.turattext.users.dto.UserSummaryResponse;
import com.turattext.websocket.WebSocketSessionRegistry;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.util.Comparator;
import java.util.List;
import java.util.UUID;

@Service
public class ChatService {
    private final ChatRepository chats;
    private final ChatMemberRepository members;
    private final UserAccountRepository users;
    private final WebSocketSessionRegistry sessions;

    public ChatService(ChatRepository chats, ChatMemberRepository members, UserAccountRepository users,
                       WebSocketSessionRegistry sessions) {
        this.chats = chats;
        this.members = members;
        this.users = users;
        this.sessions = sessions;
    }

    @Transactional
    public ChatResponse createDirect(UUID currentUserId, UUID otherUserId) {
        if (currentUserId.equals(otherUserId)) {
            throw new BadRequestException("Cannot create a direct chat with yourself");
        }

        return members.findDirectChat(currentUserId, otherUserId)
                .map(chat -> toResponse(chat, currentUserId))
                .orElseGet(() -> createNewDirect(currentUserId, otherUserId));
    }

    public List<ChatResponse> list(UUID currentUserId) {
        return members.findByUser_Id(currentUserId).stream()
                .map(ChatMember::getChat)
                .sorted(Comparator.comparing(Chat::getUpdatedAt).reversed())
                .map(chat -> toResponse(chat, currentUserId))
                .toList();
    }

    private ChatResponse createNewDirect(UUID currentUserId, UUID otherUserId) {
        UserAccount currentUser = users.findById(currentUserId)
                .orElseThrow(() -> new NotFoundException("Current user not found"));
        UserAccount otherUser = users.findById(otherUserId)
                .orElseThrow(() -> new NotFoundException("Recipient user not found"));

        Chat chat = new Chat();
        chat.setType(ChatType.DIRECT);
        Chat saved = chats.save(chat);

        members.save(member(saved, currentUser));
        members.save(member(saved, otherUser));
        return toResponse(saved, currentUserId);
    }

    private ChatMember member(Chat chat, UserAccount user) {
        ChatMember member = new ChatMember();
        member.setChat(chat);
        member.setUser(user);
        member.setRole(ChatMemberRole.MEMBER);
        return member;
    }

    private ChatResponse toResponse(Chat chat, UUID currentUserId) {
        List<UserSummaryResponse> chatMembers = members.findByChat_Id(chat.getId()).stream()
                .map(member -> toUserSummary(member.getUser()))
                .toList();
        UserSummaryResponse peer = chatMembers.stream()
                .filter(member -> !member.id().equals(currentUserId))
                .findFirst()
                .orElse(chatMembers.isEmpty() ? null : chatMembers.getFirst());
        String title = peer == null ? "Chat" : peer.displayName();
        String avatarUrl = peer == null ? null : peer.avatarUrl();
        return new ChatResponse(chat.getId(), chat.getType(), title, avatarUrl, chatMembers, chat.getUpdatedAt());
    }

    private UserSummaryResponse toUserSummary(UserAccount user) {
        UserProfile profile = user.getProfile();
        return new UserSummaryResponse(
                user.getId(),
                user.getLogin(),
                profile.getDisplayName(),
                profile.getAvatarUrl(),
                profile.getStatus(),
                profile.getDescription(),
                sessions.isOnline(user.getId())
        );
    }
}
