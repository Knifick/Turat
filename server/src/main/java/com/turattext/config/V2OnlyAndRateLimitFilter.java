package com.turattext.config;

import jakarta.servlet.FilterChain;
import jakarta.servlet.ServletException;
import jakarta.servlet.http.HttpServletRequest;
import jakarta.servlet.http.HttpServletResponse;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.core.Ordered;
import org.springframework.core.annotation.Order;
import org.springframework.stereotype.Component;
import org.springframework.web.filter.OncePerRequestFilter;

import java.io.IOException;
import java.time.Instant;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLong;

@Component
@Order(Ordered.HIGHEST_PRECEDENCE)
public class V2OnlyAndRateLimitFilter extends OncePerRequestFilter {
    private final boolean v2Only;
    private final int requestsPerMinute;
    private final boolean trustForwardedFor;
    private final Map<String, Window> windows = new ConcurrentHashMap<>();

    public V2OnlyAndRateLimitFilter(
            @Value("${turattext.v2.only:false}") boolean v2Only,
            @Value("${turattext.v2.requests-per-minute:600}") int requestsPerMinute,
            @Value("${turattext.v2.trust-forwarded-for:false}") boolean trustForwardedFor
    ) {
        this.v2Only = v2Only;
        this.requestsPerMinute = Math.clamp(requestsPerMinute, 30, 100_000);
        this.trustForwardedFor = trustForwardedFor;
    }

    @Override
    protected void doFilterInternal(
            HttpServletRequest request,
            HttpServletResponse response,
            FilterChain filterChain
    ) throws ServletException, IOException {
        String path = request.getRequestURI();
        response.setHeader("X-Content-Type-Options", "nosniff");
        response.setHeader("Referrer-Policy", "no-referrer");
        response.setHeader("Cache-Control", "no-store");
        if (v2Only && !path.startsWith("/v2/")) {
            response.setStatus(HttpServletResponse.SC_NOT_FOUND);
            return;
        }
        if (path.startsWith("/v2/") && !allow(clientAddress(request))) {
            response.setHeader("Retry-After", "60");
            response.setStatus(429);
            return;
        }
        filterChain.doFilter(request, response);
    }

    private String clientAddress(HttpServletRequest request) {
        if (!trustForwardedFor) return request.getRemoteAddr();
        String forwarded = request.getHeader("X-Forwarded-For");
        if (forwarded == null || forwarded.isBlank()) return request.getRemoteAddr();
        String first = forwarded.split(",", 2)[0].trim();
        return first.isEmpty() || first.length() > 64 ? request.getRemoteAddr() : first;
    }

    private boolean allow(String remoteAddress) {
        long minute = Instant.now().getEpochSecond() / 60;
        Window window = windows.compute(remoteAddress, (ignored, current) -> {
            if (current == null || current.minute != minute) return new Window(minute);
            return current;
        });
        boolean allowed = window.count.incrementAndGet() <= requestsPerMinute;
        if (windows.size() > 50_000) windows.entrySet().removeIf(entry -> entry.getValue().minute < minute - 2);
        return allowed;
    }

    private static final class Window {
        private final long minute;
        private final AtomicLong count = new AtomicLong();

        private Window(long minute) {
            this.minute = minute;
        }
    }
}
