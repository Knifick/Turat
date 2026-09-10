package com.turattext.chats;

import com.turattext.chats.dto.ChatResponse;
import com.turattext.chats.dto.CreateDirectChatRequest;
import com.turattext.common.SecurityUtils;
import com.turattext.messages.MessageService;
import com.turattext.messages.dto.MessageResponse;
import jakarta.validation.Valid;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.util.List;
import java.util.UUID;

@RestController
@RequestMapping("/api/chats")
public class ChatController {
    private final ChatService chatService;
    private final MessageService messageService;

    public ChatController(ChatService chatService, MessageService messageService) {
        this.chatService = chatService;
        this.messageService = messageService;
    }

    @PostMapping("/direct")
    ChatResponse createDirect(@Valid @RequestBody CreateDirectChatRequest request) {
        return chatService.createDirect(SecurityUtils.currentUserId(), request.userId());
    }

    @GetMapping
    List<ChatResponse> list() {
        return chatService.list(SecurityUtils.currentUserId());
    }

    @GetMapping("/{id}/messages")
    List<MessageResponse> messages(@PathVariable UUID id) {
        return messageService.history(SecurityUtils.currentUserId(), id);
    }
}

