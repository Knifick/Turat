package com.turattext.config;

import com.turattext.v2.CallWebSocketHandler;
import com.turattext.websocket.TuratWebSocketHandler;
import org.springframework.context.annotation.Configuration;
import org.springframework.context.annotation.Bean;
import org.springframework.web.socket.config.annotation.EnableWebSocket;
import org.springframework.web.socket.config.annotation.WebSocketConfigurer;
import org.springframework.web.socket.config.annotation.WebSocketHandlerRegistry;
import org.springframework.web.socket.server.standard.ServletServerContainerFactoryBean;

@Configuration
@EnableWebSocket
public class WebSocketConfig implements WebSocketConfigurer {
    private final TuratWebSocketHandler handler;
    private final CallWebSocketHandler callHandler;

    public WebSocketConfig(TuratWebSocketHandler handler, CallWebSocketHandler callHandler) {
        this.handler = handler;
        this.callHandler = callHandler;
    }

    @Override
    public void registerWebSocketHandlers(WebSocketHandlerRegistry registry) {
        registry.addHandler(handler, "/ws")
                .setAllowedOrigins("*");
        // Запасной транспорт звонков: голос внутри TLS на 443-м порту.
        registry.addHandler(callHandler, "/v2/calls/ws")
                .setAllowedOrigins("*");
    }

    @Bean
    ServletServerContainerFactoryBean webSocketContainer() {
        var container = new ServletServerContainerFactoryBean();
        container.setMaxTextMessageBufferSize(6 * 1024 * 1024);
        container.setMaxBinaryMessageBufferSize(6 * 1024 * 1024);
        return container;
    }
}
