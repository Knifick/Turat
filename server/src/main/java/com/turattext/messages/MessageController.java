package com.turattext.messages;

import com.fasterxml.jackson.databind.ObjectMapper;
import com.turattext.common.SecurityUtils;
import com.turattext.messages.dto.MessageStateResponse;
import com.turattext.messages.dto.PinMessageRequest;
import com.turattext.messages.dto.ReactionRequest;
import com.turattext.messages.dto.ReactionUpdateResponse;
import com.turattext.websocket.WebSocketSessionRegistry;
import jakarta.validation.Valid;
import org.springframework.http.HttpStatus;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.ResponseStatus;
import org.springframework.web.bind.annotation.RestController;

import java.util.Map;
import java.util.UUID;

@RestController
@RequestMapping("/api/messages")
public class MessageController {
    private final MessageService messages;
    private final WebSocketSessionRegistry sessions;
    private final ObjectMapper objectMapper;

    public MessageController(
            MessageService messages,
            WebSocketSessionRegistry sessions,
            ObjectMapper objectMapper
    ) {
        this.messages = messages;
        this.sessions = sessions;
        this.objectMapper = objectMapper;
    }

    @PutMapping("/{id}/reaction")
    ReactionUpdateResponse react(
            @PathVariable UUID id,
            @Valid @RequestBody ReactionRequest request
    ) {
        ReactionUpdateResponse result = messages.toggleReaction(
                SecurityUtils.currentUserId(), id, request.reaction());
        publishReactions(result.messageId(), result.chatId());
        return result;
    }

    @PutMapping("/{id}/pin")
    MessageStateResponse pin(
            @PathVariable UUID id,
            @RequestBody PinMessageRequest request
    ) {
        MessageStateResponse result = messages.setPinned(SecurityUtils.currentUserId(), id, request.pinned());
        publish(result.chatId(), "message.state", result);
        return result;
    }

    @DeleteMapping("/{id}")
    @ResponseStatus(HttpStatus.OK)
    MessageStateResponse delete(@PathVariable UUID id) {
        MessageStateResponse result = messages.delete(SecurityUtils.currentUserId(), id);
        publish(result.chatId(), "message.state", result);
        return result;
    }

    private void publishReactions(UUID messageId, UUID chatId) {
        for (UUID memberId : messages.memberIds(chatId)) {
            send(memberId, "message.reaction", messages.reactionUpdate(messageId, memberId));
        }
    }

    private void publish(UUID chatId, String type, Object payload) {
        for (UUID memberId : messages.memberIds(chatId)) {
            send(memberId, type, payload);
        }
    }

    private void send(UUID userId, String type, Object payload) {
        try {
            sessions.sendToUser(userId, objectMapper.writeValueAsString(Map.of(
                    "type", type,
                    "payload", payload
            )));
        } catch (Exception ignored) {
            // REST already succeeded; clients refresh state on reconnect.
        }
    }
}
