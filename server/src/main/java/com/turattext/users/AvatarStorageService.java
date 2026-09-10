package com.turattext.users;

import com.turattext.common.BadRequestException;
import com.turattext.common.NotFoundException;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.http.MediaType;
import org.springframework.stereotype.Service;
import org.springframework.web.multipart.MultipartFile;

import javax.imageio.ImageIO;
import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.UUID;

@Service
public class AvatarStorageService {
    private static final long MAX_BYTES = 5L * 1024 * 1024;
    private static final int MAX_DIMENSION = 4096;
    private static final long MAX_PIXELS = 16_000_000;

    private final Path avatarDirectory;

    public AvatarStorageService(@Value("${turattext.media.directory:./data/media}") String mediaDirectory) {
        avatarDirectory = Path.of(mediaDirectory).toAbsolutePath().normalize().resolve("avatars");
    }

    public String store(UUID userId, MultipartFile file) {
        if (file == null || file.isEmpty() || file.getSize() > MAX_BYTES) {
            throw new BadRequestException("Avatar must be a non-empty image up to 5 MB");
        }

        try {
            byte[] bytes = file.getBytes();
            var image = ImageIO.read(new ByteArrayInputStream(bytes));
            if (image == null) {
                throw new BadRequestException("Unsupported or damaged image");
            }
            if (image.getWidth() > MAX_DIMENSION || image.getHeight() > MAX_DIMENSION
                    || (long) image.getWidth() * image.getHeight() > MAX_PIXELS) {
                throw new BadRequestException("Avatar dimensions are too large");
            }

            String extension = detectExtension(bytes);
            Files.createDirectories(avatarDirectory);
            deleteExisting(userId);
            Path temporary = Files.createTempFile(avatarDirectory, userId + "-", ".upload");
            Files.write(temporary, bytes);
            Files.move(temporary, avatarDirectory.resolve(userId + extension),
                    StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
            return "/api/users/" + userId + "/avatar?v=" + System.currentTimeMillis();
        } catch (BadRequestException ex) {
            throw ex;
        } catch (IOException ex) {
            throw new IllegalStateException("Could not save avatar", ex);
        }
    }

    public AvatarFile load(UUID userId) {
        for (String extension : new String[]{".jpg", ".png"}) {
            Path candidate = avatarDirectory.resolve(userId + extension);
            if (Files.isRegularFile(candidate)) {
                try {
                    MediaType mediaType = extension.equals(".png") ? MediaType.IMAGE_PNG : MediaType.IMAGE_JPEG;
                    return new AvatarFile(Files.readAllBytes(candidate), mediaType);
                } catch (IOException ex) {
                    throw new IllegalStateException("Could not read avatar", ex);
                }
            }
        }
        throw new NotFoundException("Avatar not found");
    }

    private String detectExtension(byte[] bytes) {
        if (bytes.length >= 8
                && (bytes[0] & 0xff) == 0x89 && bytes[1] == 0x50 && bytes[2] == 0x4e && bytes[3] == 0x47) {
            return ".png";
        }
        if (bytes.length >= 3
                && (bytes[0] & 0xff) == 0xff && (bytes[1] & 0xff) == 0xd8 && (bytes[2] & 0xff) == 0xff) {
            return ".jpg";
        }
        throw new BadRequestException("Only JPEG and PNG avatars are supported");
    }

    private void deleteExisting(UUID userId) throws IOException {
        Files.deleteIfExists(avatarDirectory.resolve(userId + ".jpg"));
        Files.deleteIfExists(avatarDirectory.resolve(userId + ".png"));
    }

    public record AvatarFile(byte[] bytes, MediaType mediaType) {
    }
}
