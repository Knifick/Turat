package com.turattext.messages;

import com.turattext.chats.Chat;
import com.turattext.chats.ChatMemberRepository;
import com.turattext.chats.ChatRepository;
import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import com.turattext.messages.dto.MessageResponse;
import com.turattext.messages.dto.MessageReactionResponse;
import com.turattext.messages.dto.MessageStateResponse;
import com.turattext.messages.dto.ReactionUpdateResponse;
import com.turattext.messages.dto.SendMessageRequest;
import com.turattext.users.UserAccount;
import com.turattext.users.UserAccountRepository;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.time.Instant;
import java.util.LinkedHashMap;

@Service
public class MessageService {
    private final MessageRepository messages;
    private final ChatRepository chats;
    private final ChatMemberRepository members;
    private final UserAccountRepository users;
    private final MessageReactionRepository reactions;
    private static final Set<String> ALLOWED_REACTIONS = Set.of(
            "❤", "🔥", "👌", "😱", "😭", "🤨", "👍", "💔"
    );

    public MessageService(
            MessageRepository messages,
            ChatRepository chats,
            ChatMemberRepository members,
            UserAccountRepository users,
            MessageReactionRepository reactions
    ) {
        this.messages = messages;
        this.chats = chats;
        this.members = members;
        this.users = users;
        this.reactions = reactions;
    }

    public List<MessageResponse> history(UUID currentUserId, UUID chatId) {
        requireMember(chatId, currentUserId);
        return messages.findByChat_IdOrderByCreatedAtAsc(chatId).stream()
                .filter(message -> message.getStatus() != MessageStatus.DELETED)
                .map(message -> toResponse(message, currentUserId))
                .toList();
    }

    @Transactional
    public MessageResponse saveFromUser(UUID senderId, SendMessageRequest request) {
        requireMember(request.chatId(), senderId);
        Chat chat = chats.findById(request.chatId())
                .orElseThrow(() -> new NotFoundException("Chat not found"));
        UserAccount sender = users.findById(senderId)
                .orElseThrow(() -> new NotFoundException("Sender not found"));

        Message message = new Message();
        message.setChat(chat);
        message.setSender(sender);
        message.setEncryptedContent(request.encryptedContent());
        message.setNonce(request.nonce());
        message.setEncryptionKeyId(request.encryptionKeyId());
        if (request.replyToMessageId() != null) {
            Message replied = messages.findById(request.replyToMessageId())
                    .orElseThrow(() -> new BadRequestException("Reply target was not found"));
            if (!replied.getChat().getId().equals(request.chatId())
                    || replied.getStatus() == MessageStatus.DELETED) {
                throw new BadRequestException("Reply target does not belong to this chat");
            }
            message.setReplyToMessageId(replied.getId());
        }
        return toResponse(messages.save(message), senderId);
    }

    @Transactional
    public ReactionUpdateResponse toggleReaction(UUID userId, UUID messageId, String value) {
        String reactionValue = value == null ? "" : value.trim();
        if (!ALLOWED_REACTIONS.contains(reactionValue)) {
            throw new BadRequestException("Unsupported reaction");
        }
        Message message = requireMessageMember(messageId, userId);
        MessageReactionId id = new MessageReactionId(messageId, userId);
        MessageReaction reaction = reactions.findById(id).orElse(null);
        if (reaction == null) {
            reaction = new MessageReaction();
            reaction.setId(id);
            reaction.setMessage(message);
            reaction.setUser(users.findById(userId)
                    .orElseThrow(() -> new NotFoundException("User not found")));
            reaction.setReaction(reactionValue);
            reaction.setActive(true);
        } else if (reaction.isActive() && reaction.getReaction().equals(reactionValue)) {
            reaction.setActive(false);
        } else {
            reaction.setReaction(reactionValue);
            reaction.setActive(true);
        }
        reaction.setUpdatedAt(Instant.now());
        reactions.save(reaction);
        return reactionUpdate(messageId, userId);
    }

    public ReactionUpdateResponse reactionUpdate(UUID messageId, UUID viewerId) {
        Message message = messages.findById(messageId)
                .orElseThrow(() -> new NotFoundException("Message not found"));
        return new ReactionUpdateResponse(
                messageId,
                message.getChat().getId(),
                summarizeReactions(messageId, viewerId)
        );
    }

    @Transactional
    public MessageStateResponse setPinned(UUID userId, UUID messageId, boolean pinned) {
        Message message = requireMessageMember(messageId, userId);
        message.setPinned(pinned);
        message.setUpdatedAt(Instant.now());
        messages.save(message);
        return new MessageStateResponse(messageId, message.getChat().getId(), pinned, false);
    }

    @Transactional
    public MessageStateResponse delete(UUID userId, UUID messageId) {
        Message message = requireMessageMember(messageId, userId);
        if (!message.getSender().getId().equals(userId)) {
            throw new BadRequestException("Only the sender can delete this message");
        }
        message.setStatus(MessageStatus.DELETED);
        message.setPinned(false);
        message.setEncryptedContent("");
        message.setNonce("");
        message.setEncryptionKeyId("");
        message.setUpdatedAt(Instant.now());
        messages.save(message);
        for (MessageReaction reaction : reactions.findByMessage_IdAndActiveTrue(messageId)) {
            reaction.setActive(false);
            reaction.setUpdatedAt(Instant.now());
            reactions.save(reaction);
        }
        return new MessageStateResponse(messageId, message.getChat().getId(), false, true);
    }

    public List<UUID> memberIds(UUID chatId) {
        return members.findMemberIds(chatId);
    }

    private Message requireMessageMember(UUID messageId, UUID userId) {
        Message message = messages.findById(messageId)
                .orElseThrow(() -> new NotFoundException("Message not found"));
        requireMember(message.getChat().getId(), userId);
        if (message.getStatus() == MessageStatus.DELETED) {
            throw new BadRequestException("Message was deleted");
        }
        return message;
    }

    private void requireMember(UUID chatId, UUID userId) {
        if (!members.existsByChat_IdAndUser_Id(chatId, userId)) {
            throw new BadRequestException("User is not a member of this chat");
        }
    }

    private MessageResponse toResponse(Message message, UUID viewerId) {
        return new MessageResponse(
                message.getId(),
                message.getChat().getId(),
                message.getSender().getId(),
                message.getEncryptedContent(),
                message.getNonce(),
                message.getEncryptionKeyId(),
                message.getStatus(),
                message.getCreatedAt(),
                message.getReplyToMessageId(),
                message.isPinned(),
                summarizeReactions(message.getId(), viewerId)
        );
    }

    private List<MessageReactionResponse> summarizeReactions(UUID messageId, UUID viewerId) {
        Map<String, ReactionAccumulator> grouped = new LinkedHashMap<>();
        for (MessageReaction reaction : reactions.findByMessage_IdAndActiveTrue(messageId)) {
            ReactionAccumulator accumulator = grouped.computeIfAbsent(
                    reaction.getReaction(), ignored -> new ReactionAccumulator());
            accumulator.count++;
            if (reaction.getUser().getId().equals(viewerId)) accumulator.reactedByMe = true;
        }
        return grouped.entrySet().stream()
                .map(entry -> new MessageReactionResponse(
                        entry.getKey(), entry.getValue().count, entry.getValue().reactedByMe))
                .toList();
    }

    private static final class ReactionAccumulator {
        private long count;
        private boolean reactedByMe;
    }
}
