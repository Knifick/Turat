package com.turattext.websocket;

import com.fasterxml.jackson.databind.JsonNode;

public record WebSocketEnvelope(String type, JsonNode payload) {
}

