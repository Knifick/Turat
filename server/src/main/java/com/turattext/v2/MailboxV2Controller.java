package com.turattext.v2;

import jakarta.validation.Valid;
import jakarta.validation.constraints.NotBlank;
import jakarta.validation.constraints.NotEmpty;
import jakarta.validation.constraints.NotNull;
import jakarta.validation.constraints.Size;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestHeader;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;
import org.springframework.web.context.request.async.DeferredResult;

import java.time.Instant;
import java.util.List;
import java.util.UUID;

@RestController
@RequestMapping("/v2/mailboxes")
public class MailboxV2Controller {
    private final MailboxV2Service mailboxes;

    public MailboxV2Controller(MailboxV2Service mailboxes) {
        this.mailboxes = mailboxes;
    }

    @PostMapping
    public MailboxV2Service.MailboxRegistration register(@Valid @RequestBody RegisterMailboxRequest request) {
        return mailboxes.register(new MailboxV2Service.RegisterMailbox(
                request.readCapability(), request.writeCapability(), request.contactCapability(), request.deviceHint(),
                request.expiresAt(), request.proofNonce()));
    }

    @PutMapping("/{mailboxId}/envelopes")
    public MailboxV2Service.StoredEnvelope put(
            @PathVariable UUID mailboxId,
            @RequestHeader("X-Mailbox-Write-Capability") String capability,
            @RequestHeader(value = "X-Envelope-Pow-Nonce", required = false) String proofNonce,
            @Valid @RequestBody PutEnvelopeRequest request
    ) {
        return mailboxes.put(mailboxId, capability, proofNonce, new MailboxV2Service.PutEnvelope(
                request.envelopeId(), request.protocolVersion(), request.recipientDeviceHint(),
                request.opaquePayload(), request.sizeClass(), request.createdAt(), request.expiresAt()));
    }

    /**
     * Выдача конвертов. При {@code wait > 0} Node держит запрос открытым до появления конверта:
     * так сообщение доходит сразу, а не к следующему циклу опроса у клиента.
     */
    @GetMapping("/{mailboxId}/envelopes")
    public DeferredResult<List<MailboxV2Service.StoredEnvelope>> fetch(
            @PathVariable UUID mailboxId,
            @RequestHeader("X-Mailbox-Read-Capability") String capability,
            @RequestParam(defaultValue = "100") int limit,
            @RequestParam(defaultValue = "0") int wait
    ) {
        return mailboxes.fetchOrWait(mailboxId, capability, limit, wait);
    }

    @PostMapping("/{mailboxId}/ack")
    public AcknowledgeResponse acknowledge(
            @PathVariable UUID mailboxId,
            @RequestHeader("X-Mailbox-Read-Capability") String capability,
            @Valid @RequestBody AcknowledgeRequest request
    ) {
        return new AcknowledgeResponse(mailboxes.acknowledge(mailboxId, capability, request.envelopeIds()));
    }

    public record RegisterMailboxRequest(
            @NotBlank @Size(min = 32, max = 256) String readCapability,
            @NotBlank @Size(min = 32, max = 256) String writeCapability,
            @NotBlank @Size(min = 32, max = 256) String contactCapability,
            @NotBlank @Size(max = 160) String deviceHint,
            Instant expiresAt,
            @Size(max = 100) String proofNonce
    ) {
    }

    public record PutEnvelopeRequest(
            @NotBlank @Size(min = 16, max = 100) String envelopeId,
            int protocolVersion,
            @Size(max = 160) String recipientDeviceHint,
            @NotBlank String opaquePayload,
            Integer sizeClass,
            Instant createdAt,
            Instant expiresAt
    ) {
    }

    public record AcknowledgeRequest(
            @NotEmpty @Size(max = 200) List<@NotBlank @Size(max = 100) String> envelopeIds
    ) {
    }

    public record AcknowledgeResponse(int deleted) {
    }
}
