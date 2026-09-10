package com.turattext.messages.dto;

import java.util.List;
import java.util.UUID;

public record ReactionUpdateResponse(
        UUID messageId,
        UUID chatId,
        List<MessageReactionResponse> reactions
) {
}
