package org.rsharemouse.mobile;

import static org.junit.Assert.assertTrue;
import static org.junit.Assert.assertFalse;

import java.util.List;
import org.junit.Test;

public final class DiscoveryTargetsTest {
    @Test
    public void scansLocalNetworkBeforeCommonRoutedHomeNetwork() {
        List<String> targets = DiscoveryTargets.forPrivateIpv4("192.168.10.102");
        assertTrue(targets.contains("192.168.10.253"));
        assertTrue(targets.contains("192.168.1.253"));
        assertTrue(targets.indexOf("192.168.10.253") < targets.indexOf("192.168.1.253"));
        assertFalse(targets.contains("192.168.10.102"));
    }

    @Test
    public void leavesOtherPrivateNetworksOnTheirLocalSubnet() {
        List<String> targets = DiscoveryTargets.forPrivateIpv4("10.0.2.16");
        assertTrue(targets.contains("10.0.2.15"));
        assertFalse(targets.contains("192.168.1.253"));
    }

    @Test
    public void emulatorAddsLoopbackForAdbReverseWithoutChangingDeviceTargets() {
        List<String> physical = DiscoveryTargets.withEmulatorLoopback(
                DiscoveryTargets.forPrivateIpv4("10.0.2.16"), false);
        List<String> emulator = DiscoveryTargets.withEmulatorLoopback(
                DiscoveryTargets.forPrivateIpv4("10.0.2.16"), true);

        assertFalse(physical.contains("127.0.0.1"));
        assertTrue(emulator.contains("127.0.0.1"));
        assertTrue(emulator.indexOf("127.0.0.1") < emulator.indexOf("10.0.2.2"));
    }
}
