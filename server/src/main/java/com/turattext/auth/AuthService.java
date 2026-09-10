package com.turattext.auth;

import com.turattext.auth.dto.AuthResponse;
import com.turattext.auth.dto.LoginRequest;
import com.turattext.auth.dto.RefreshRequest;
import com.turattext.auth.dto.RegisterRequest;
import com.turattext.common.BadRequestException;
import com.turattext.common.UnauthorizedException;
import com.turattext.users.UserAccount;
import com.turattext.users.UserAccountRepository;
import com.turattext.users.UserProfile;
import com.turattext.users.UserPublicKey;
import com.turattext.users.UserPublicKeyRepository;
import org.springframework.security.crypto.password.PasswordEncoder;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.util.UUID;

@Service
public class AuthService {
    private final UserAccountRepository users;
    private final UserPublicKeyRepository publicKeys;
    private final PasswordEncoder passwordEncoder;
    private final JwtService jwtService;

    public AuthService(
            UserAccountRepository users,
            UserPublicKeyRepository publicKeys,
            PasswordEncoder passwordEncoder,
            JwtService jwtService
    ) {
        this.users = users;
        this.publicKeys = publicKeys;
        this.passwordEncoder = passwordEncoder;
        this.jwtService = jwtService;
    }

    @Transactional
    public AuthResponse register(RegisterRequest request) {
        if (users.existsByLoginIgnoreCase(request.login())) {
            throw new BadRequestException("Login is already taken");
        }

        UserAccount user = new UserAccount();
        user.setLogin(request.login());
        user.setPasswordHash(passwordEncoder.encode(request.password()));

        UserProfile profile = new UserProfile();
        profile.setDisplayName(blankToDefault(request.displayName(), request.login()));
        profile.setStatus("online");
        user.setProfile(profile);

        UserAccount saved = users.save(user);
        if (request.publicKey() != null && !request.publicKey().isBlank()) {
            UserPublicKey key = new UserPublicKey();
            key.setUser(saved);
            key.setKeyId("identity-" + UUID.randomUUID());
            key.setAlgorithm("ECDH-P256-AESGCM-MVP");
            key.setPublicKey(request.publicKey());
            publicKeys.save(key);
        }

        return response(saved);
    }

    public AuthResponse login(LoginRequest request) {
        UserAccount user = users.findByLoginIgnoreCase(request.login())
                .orElseThrow(() -> new UnauthorizedException("Invalid login or password"));
        if (!user.isEnabled() || !passwordEncoder.matches(request.password(), user.getPasswordHash())) {
            throw new UnauthorizedException("Invalid login or password");
        }
        return response(user);
    }

    public AuthResponse refresh(RefreshRequest request) {
        if (!"refresh".equals(jwtService.requireTokenType(request.refreshToken()))) {
            throw new UnauthorizedException("Refresh token is required");
        }
        UUID userId = jwtService.requireUserId(request.refreshToken());
        UserAccount user = users.findById(userId)
                .orElseThrow(() -> new UnauthorizedException("User not found"));
        if (!user.isEnabled()) {
            throw new UnauthorizedException("User is disabled");
        }
        return response(user);
    }

    private AuthResponse response(UserAccount user) {
        return new AuthResponse(
                jwtService.createAccessToken(user.getId(), user.getLogin()),
                jwtService.createRefreshToken(user.getId(), user.getLogin()),
                user.getId(),
                user.getLogin(),
                user.getProfile().getDisplayName(),
                user.getCreatedAt()
        );
    }

    private static String blankToDefault(String value, String fallback) {
        return value == null || value.isBlank() ? fallback : value;
    }
}
