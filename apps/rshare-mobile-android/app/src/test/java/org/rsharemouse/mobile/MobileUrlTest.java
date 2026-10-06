package org.rsharemouse.mobile;

import static org.junit.Assert.assertEquals;
import static org.junit.Assert.assertNull;

import org.junit.Test;

public final class MobileUrlTest {
    @Test
    public void acceptsPrivateLanGatewayUrl() {
        assertEquals(
                "http://192.168.1.253:27437/mobile?t=abc%2F123",
                MobileUrl.parse("  http://192.168.1.253:27437/mobile?t=abc%2F123  "));
    }

    @Test
    public void rejectsUntrustedOrUnusableUrls() {
        assertNull(MobileUrl.parse("https://192.168.1.253:27437/mobile?t=token"));
        assertNull(MobileUrl.parse("http://example.com:27437/mobile?t=token"));
        assertNull(MobileUrl.parse("http://8.8.8.8:27437/mobile?t=token"));
        assertNull(MobileUrl.parse("http://192.168.1.253:1234/mobile?t=token"));
        assertNull(MobileUrl.parse("http://192.168.1.253:27437/other?t=token"));
        assertNull(MobileUrl.parse("http://192.168.1.253:27437/mobile"));
        assertNull(MobileUrl.parse("http://192.168.1.253:27437/mobile?t="));
        assertNull(MobileUrl.parse("http://user:pass@192.168.1.253:27437/mobile?t=token"));
        assertNull(MobileUrl.parse("http://192.168.1.253:27437/mobile?t=token#fragment"));
    }
}
