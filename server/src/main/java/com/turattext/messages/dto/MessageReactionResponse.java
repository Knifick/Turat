package com.turattext.messages.dto;

public record MessageReactionResponse(String reaction, long count, boolean reactedByMe) {
}
