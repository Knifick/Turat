package com.turattext.v2;

import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/v2")
public class NodeV2Controller {
    private final NodeIdentityService identity;

    public NodeV2Controller(NodeIdentityService identity) {
        this.identity = identity;
    }

    @GetMapping("/node-descriptor")
    public NodeIdentityService.NodeDescriptor descriptor() {
        return identity.descriptor();
    }
}
