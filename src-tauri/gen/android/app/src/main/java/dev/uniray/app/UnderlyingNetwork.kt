package dev.uniray.app

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import java.net.Inet4Address

/**
 * The real network under the VPN: its kind and its DNS servers.
 *
 * On mobile data the operator's DNS may be the only one that answers (e.g.
 * while access is limited to an allow-list), so the core resolves the VPN
 * servers through it there.
 */
object UnderlyingNetwork {
  /** [address]: this device's IPv4 address in it, for the TV import page. */
  class Info(val type: String, val dns: List<String>, val address: String? = null)

  fun current(context: Context): Info {
    val manager = context.getSystemService(ConnectivityManager::class.java) ?: return Info("none", emptyList())
    @Suppress("DEPRECATION")
    val candidates =
      try {
        manager.allNetworks.mapNotNull { network ->
          val caps = manager.getNetworkCapabilities(network) ?: return@mapNotNull null
          if (
            !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) ||
              !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
          ) {
            return@mapNotNull null
          }
          Triple(network, caps, rank(caps))
        }
      } catch (e: Exception) {
        emptyList()
      }
    // Android itself prefers a working Wi-Fi or Ethernet over mobile data.
    val best =
      candidates.minWithOrNull(
        compareBy(
          { !it.second.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED) },
          { it.third },
        )
      ) ?: return Info("none", emptyList())

    val type =
      when {
        best.second.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
        best.second.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
        best.second.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
        else -> "other"
      }
    val servers =
      try {
        manager.getLinkProperties(best.first)?.dnsServers.orEmpty()
      } catch (e: Exception) {
        emptyList()
      }
    // IPv4 first: the tunnel itself is IPv4.
    val dns =
      servers
        .sortedBy { if (it is Inet4Address) 0 else 1 }
        .mapNotNull { it.hostAddress?.substringBefore('%') }
    val address =
      try {
        manager.getLinkProperties(best.first)?.linkAddresses.orEmpty()
          .map { it.address }
          .firstOrNull { it is Inet4Address && !it.isLoopbackAddress && !it.isLinkLocalAddress }
          ?.hostAddress
      } catch (e: Exception) {
        null
      }
    return Info(type, dns, address)
  }

  private fun rank(caps: NetworkCapabilities): Int =
    when {
      caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> 0
      caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> 1
      caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> 2
      else -> 3
    }
}
