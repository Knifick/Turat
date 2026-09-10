package com.turattext.users;

import com.turattext.common.SecurityUtils;
import com.turattext.users.dto.UpdateProfileRequest;
import com.turattext.users.dto.UserSummaryResponse;
import jakarta.validation.Valid;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.PatchMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.bind.annotation.RequestPart;
import org.springframework.web.multipart.MultipartFile;

import java.util.List;
import java.util.UUID;

@RestController
@RequestMapping("/api/users")
public class UserController {
    private final UserService userService;
    private final AvatarStorageService avatars;

    public UserController(UserService userService, AvatarStorageService avatars) {
        this.userService = userService;
        this.avatars = avatars;
    }

    @GetMapping("/search")
    List<UserSummaryResponse> search(@RequestParam("q") String query) {
        return userService.search(query);
    }

    @GetMapping("/{id}")
    UserSummaryResponse getPublicProfile(@PathVariable UUID id) {
        return userService.getPublicProfile(id);
    }

    @PatchMapping("/me")
    UserSummaryResponse updateMe(@Valid @RequestBody UpdateProfileRequest request) {
        return userService.updateProfile(SecurityUtils.currentUserId(), request);
    }

    @PostMapping(value = "/me/avatar", consumes = "multipart/form-data")
    UserSummaryResponse uploadAvatar(@RequestPart("file") MultipartFile file) {
        UUID userId = SecurityUtils.currentUserId();
        return userService.updateAvatar(userId, avatars.store(userId, file));
    }

    @GetMapping("/{id}/avatar")
    ResponseEntity<byte[]> avatar(@PathVariable UUID id) {
        AvatarStorageService.AvatarFile avatar = avatars.load(id);
        return ResponseEntity.ok()
                .contentType(avatar.mediaType())
                .header("Cache-Control", "private, max-age=300")
                .body(avatar.bytes());
    }
}
