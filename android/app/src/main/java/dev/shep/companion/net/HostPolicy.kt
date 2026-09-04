package dev.shep.companion.net

/**
 * Whether a bridge URL may go out unencrypted.
 *
 * The bridge speaks plain `ws://` because it lives on the tailnet, where
 * WireGuard is the encryption, or on the LAN. The manifest therefore has to
 * allow cleartext, and Android's network security config cannot say "only to
 * private addresses" — it takes host names, not ranges. So the rule lives
 * here: `wss://` always; `ws://` only to loopback, the emulator's host alias,
 * RFC 1918, the CGNAT block Tailscale hands out, link-local, IPv6 ULA and
 * link-local, and names that cannot leave the local network (`.local`,
 * `.ts.net`, `.internal`, `.lan`, `.home.arpa`, or a bare hostname). A typo
 * that points the token at a public address is refused before the token is
 * sent.
 */
fun plaintextAllowed(url: String): Boolean {
    val schemeEnd = url.indexOf("://")
    if (schemeEnd <= 0) return false
    val scheme = url.substring(0, schemeEnd).lowercase()
    if (scheme == "wss" || scheme == "https") return true
    if (scheme != "ws" && scheme != "http") return false
    val host = hostOf(url.substring(schemeEnd + 3)) ?: return false
    return isPrivateHost(host)
}

/** The host part of `authority/path`, without port, IPv6 brackets stripped. */
internal fun hostOf(rest: String): String? {
    val authority = rest.substringBefore('/').substringBefore('?').substringAfter('@')
    if (authority.isEmpty()) return null
    if (authority.startsWith("[")) {
        val end = authority.indexOf(']')
        return if (end > 1) authority.substring(1, end) else null
    }
    return authority.substringBefore(':').ifEmpty { null }
}

internal fun isPrivateHost(rawHost: String): Boolean {
    val host = rawHost.lowercase().trimEnd('.')
    if (host == "localhost") return true
    ipv4Octets(host)?.let { return isPrivateIpv4(it) }
    if (host.contains(':')) return isPrivateIpv6(host)
    if (!host.contains('.')) return true
    return listOf(".local", ".ts.net", ".internal", ".lan", ".home.arpa")
        .any { host.endsWith(it) }
}

private fun ipv4Octets(host: String): IntArray? {
    val parts = host.split('.')
    if (parts.size != 4) return null
    val octets = IntArray(4)
    for ((i, part) in parts.withIndex()) {
        val value = part.toIntOrNull() ?: return null
        if (value !in 0..255 || part.isEmpty()) return null
        octets[i] = value
    }
    return octets
}

private fun isPrivateIpv4(o: IntArray): Boolean = when {
    o[0] == 127 -> true
    o[0] == 10 -> true
    o[0] == 192 && o[1] == 168 -> true
    o[0] == 172 && o[1] in 16..31 -> true
    o[0] == 100 && o[1] in 64..127 -> true
    o[0] == 169 && o[1] == 254 -> true
    else -> false
}

private fun isPrivateIpv6(host: String): Boolean {
    val h = host.substringBefore('%')
    if (h == "::1") return true
    val head = h.substringBefore(':').lowercase()
    if (head.length > 4) return false
    val first = head.toIntOrNull(16) ?: return false
    // fc00::/7 (unique local) and fe80::/10 (link-local).
    return (first and 0xfe00) == 0xfc00 || (first and 0xffc0) == 0xfe80
}
