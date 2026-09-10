package com.turattext.users;

import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import com.turattext.users.dto.EncryptedKeyBackupRequest;
import com.turattext.users.dto.EncryptedKeyBackupResponse;
import com.turattext.users.dto.PublicKeyRequest;
import com.turattext.users.dto.PublicKeyResponse;
import com.turattext.users.dto.UpdateProfileRequest;
import com.turattext.users.dto.UserSummaryResponse;
import com.turattext.websocket.WebSocketSessionRegistry;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

import java.util.List;
import java.util.UUID;

@Service
public class UserService {
    private final UserAccountRepository users;
    private final UserProfileRepository profiles;
    private final UserPublicKeyRepository publicKeys;
    private final EncryptedKeyBackupRepository backups;
    private final WebSocketSessionRegistry sessions;

    public UserService(
            UserAccountRepository users,
            UserProfileRepository profiles,
            UserPublicKeyRepository publicKeys,
            EncryptedKeyBackupRepository backups,
            WebSocketSessionRegistry sessions
    ) {
        this.users = users;
        this.profiles = profiles;
        this.publicKeys = publicKeys;
        this.backups = backups;
        this.sessions = sessions;
    }

    public List<UserSummaryResponse> search(String query) {
        if (query == null || query.isBlank()) {
            return List.of();
        }
        return profiles.search(query.trim()).stream()
                .limit(50)
                .map(this::toSummary)
                .toList();
    }

    public UserSummaryResponse getPublicProfile(UUID userId) {
        UserProfile profile = profiles.findById(userId)
                .orElseThrow(() -> new NotFoundException("User not found"));
        return toSummary(profile);
    }

    @Transactional
    public UserSummaryResponse updateProfile(UUID userId, UpdateProfileRequest request) {
        UserProfile profile = profiles.findById(userId)
                .orElseThrow(() -> new NotFoundException("User not found"));
        if (request.displayName() != null) {
            profile.setDisplayName(request.displayName());
        }
        if (request.status() != null) {
            profile.setStatus(request.status());
        }
        if (request.description() != null) {
            profile.setDescription(request.description());
        }
        return toSummary(profile);
    }

    @Transactional
    public UserSummaryResponse updateAvatar(UUID userId, String avatarUrl) {
        UserProfile profile = profiles.findById(userId)
                .orElseThrow(() -> new NotFoundException("User not found"));
        profile.setAvatarUrl(avatarUrl);
        return toSummary(profile);
    }

    @Transactional
    public PublicKeyResponse addPublicKey(UUID userId, PublicKeyRequest request) {
        UserAccount user = users.findById(userId)
                .orElseThrow(() -> new NotFoundException("User not found"));
        var existing = publicKeys.findByKeyId(request.keyId());
        if (existing.isPresent()) {
            UserPublicKey selected = existing.get();
            if (!selected.getUser().getId().equals(userId)) {
                throw new BadRequestException("Public key id already belongs to another user");
            }
            if (!selected.getAlgorithm().equals(request.algorithm())
                    || !selected.getPublicKey().equals(request.publicKey())) {
                throw new BadRequestException("Public key does not match its existing id");
            }
            activateOnly(userId, selected);
            return toPublicKey(selected);
        }
        publicKeys.findAllByUser_IdAndRevokedAtIsNull(userId).forEach(UserPublicKey::revoke);
        UserPublicKey key = new UserPublicKey();
        key.setUser(user);
        key.setKeyId(request.keyId());
        key.setAlgorithm(request.algorithm());
        key.setPublicKey(request.publicKey());
        return toPublicKey(publicKeys.save(key));
    }

    public PublicKeyResponse getActivePublicKey(UUID userId) {
        UserPublicKey key = publicKeys.findFirstByUser_IdAndRevokedAtIsNullOrderByCreatedAtDesc(userId)
                .orElseThrow(() -> new NotFoundException("Active public key not found"));
        return toPublicKey(key);
    }

    @Transactional
    public void saveEncryptedBackup(UUID userId, EncryptedKeyBackupRequest request) {
        UserAccount user = users.findById(userId)
                .orElseThrow(() -> new NotFoundException("User not found"));
        EncryptedKeyBackup backup = new EncryptedKeyBackup();
        backup.setUser(user);
        backup.setKeyId(request.keyId());
        backup.setAlgorithm(request.algorithm());
        backup.setPublicKey(request.publicKey());
        backup.setSalt(request.salt());
        backup.setNonce(request.nonce());
        backup.setKdf(request.kdf());
        backup.setEncryptedPrivateKey(request.encryptedPrivateKey());
        backups.deleteByUser_Id(userId);
        backups.save(backup);
    }

    public EncryptedKeyBackupResponse getEncryptedBackup(UUID userId) {
        EncryptedKeyBackup backup = backups.findFirstByUser_IdOrderByCreatedAtDesc(userId)
                .orElseThrow(() -> new NotFoundException("Encrypted key backup not found"));
        return new EncryptedKeyBackupResponse(
                backup.getKeyId(),
                backup.getAlgorithm(),
                backup.getPublicKey(),
                backup.getSalt(),
                backup.getNonce(),
                backup.getKdf(),
                backup.getEncryptedPrivateKey(),
                backup.getCreatedAt()
        );
    }

    private void activateOnly(UUID userId, UserPublicKey selected) {
        publicKeys.findAllByUser_IdAndRevokedAtIsNull(userId).stream()
                .filter(key -> !key.getId().equals(selected.getId()))
                .forEach(UserPublicKey::revoke);
        selected.reactivate();
    }

    private UserSummaryResponse toSummary(UserProfile profile) {
        return new UserSummaryResponse(
                profile.getUser().getId(),
                profile.getUser().getLogin(),
                profile.getDisplayName(),
                profile.getAvatarUrl(),
                profile.getStatus(),
                profile.getDescription(),
                sessions.isOnline(profile.getUser().getId())
        );
    }

    private PublicKeyResponse toPublicKey(UserPublicKey key) {
        return new PublicKeyResponse(
                key.getId(),
                key.getUser().getId(),
                key.getKeyId(),
                key.getAlgorithm(),
                key.getPublicKey(),
                key.getCreatedAt()
        );
    }
}
