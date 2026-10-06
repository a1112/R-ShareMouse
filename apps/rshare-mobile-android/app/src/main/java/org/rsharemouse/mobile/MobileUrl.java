package org.rsharemouse.mobile;

import java.net.URI;
import java.net.URISyntaxException;
import java.net.URLDecoder;
import java.nio.charset.StandardCharsets;

final class MobileUrl {
    private MobileUrl() {}

    static String parse(String input) {
        if (input == null) {
            return null;
        }
        String value = input.trim();
        try {
            URI uri = new URI(value);
            if (!"http".equalsIgnoreCase(uri.getScheme())
                    || uri.getRawUserInfo() != null
                    || uri.getRawFragment() != null
                    || uri.getPort() != 27437
                    || !"/mobile".equals(uri.getRawPath())
                    || !isPrivateIpv4(uri.getHost())) {
                return null;
            }
            String query = uri.getRawQuery();
            if (query == null || !query.startsWith("t=") || query.contains("&")) {
                return null;
            }
            String token = URLDecoder.decode(query.substring(2), StandardCharsets.UTF_8.name());
            return token.isEmpty() ? null : value;
        } catch (URISyntaxException | IllegalArgumentException exception) {
            return null;
        } catch (java.io.UnsupportedEncodingException exception) {
            throw new AssertionError(exception);
        }
    }

    private static boolean isPrivateIpv4(String host) {
        if (host == null) {
            return false;
        }
        String[] parts = host.split("\\.", -1);
        if (parts.length != 4) {
            return false;
        }
        int[] octets = new int[4];
        for (int i = 0; i < parts.length; i++) {
            if (parts[i].isEmpty() || parts[i].length() > 3) {
                return false;
            }
            for (int j = 0; j < parts[i].length(); j++) {
                if (!Character.isDigit(parts[i].charAt(j))) {
                    return false;
                }
            }
            octets[i] = Integer.parseInt(parts[i]);
            if (octets[i] > 255) {
                return false;
            }
        }
        return octets[0] == 10
                || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
                || (octets[0] == 192 && octets[1] == 168)
                || octets[0] == 127;
    }
}
