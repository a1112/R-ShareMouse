package org.rsharemouse.mobile;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

final class DiscoveryTargets {
    private DiscoveryTargets() { }

    static List<String> forPrivateIpv4(String address) {
        String[] octets = address.split("\\.");
        if (octets.length != 4) return new ArrayList<>();
        int[] parsed = new int[4];
        try {
            for (int i = 0; i < 4; i++) {
                parsed[i] = Integer.parseInt(octets[i]);
                if (parsed[i] < 0 || parsed[i] > 255) return new ArrayList<>();
            }
        } catch (NumberFormatException error) {
            return new ArrayList<>();
        }

        Set<String> targets = new LinkedHashSet<>();
        addSubnet(targets, parsed[0] + "." + parsed[1] + "." + parsed[2] + ".", address);
        if (parsed[0] == 192 && parsed[1] == 168) {
            // Home routers commonly put wired and Wi-Fi clients on different routed /24s.
            addSubnet(targets, "192.168.1.", address);
            addSubnet(targets, "192.168.0.", address);
            addSubnet(targets, "192.168.10.", address);
        }
        return new ArrayList<>(targets);
    }

    static List<String> withEmulatorLoopback(List<String> subnetTargets, boolean emulator) {
        Set<String> targets = new LinkedHashSet<>();
        if (emulator) targets.add("127.0.0.1");
        targets.addAll(subnetTargets);
        return new ArrayList<>(targets);
    }

    private static void addSubnet(Set<String> targets, String prefix, String ownAddress) {
        for (int host = 1; host < 255; host++) {
            String candidate = prefix + host;
            if (!candidate.equals(ownAddress)) targets.add(candidate);
        }
    }
}
