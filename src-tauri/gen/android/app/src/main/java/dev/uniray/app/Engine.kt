package dev.uniray.app

import android.content.Context
import android.net.ConnectivityManager
import android.net.LocalServerSocket
import android.net.LocalSocket
import android.os.Build
import android.os.Process
import android.system.Os
import android.system.OsConstants
import android.util.Log
import java.io.File
import java.net.InetAddress
import java.net.InetSocketAddress
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.json.JSONObject

/**
 * `uniray-engine`, which runs the servers Xray cannot (WireGuard, AmneziaWG, Naive)
 * and serves them to Xray, and to nothing else, as a SOCKS5 proxy on loopback
 * (see `engine/` and `uniray_core::engine` in the repository).
 *
 * It ships as a native library so that it is extracted where an app may run
 * programs from: Android forbids executing files in the app's data directory.
 */
object Engine {
  private const val TAG = "UniRayEngine"
  private const val READY_TIMEOUT_MS = 10_000L

  fun binary(context: Context) = File(context.applicationInfo.nativeLibraryDir, "libuniray_engine.so")

  /**
   * The engine's config with the `naive` it runs for a Naive server, which
   * ships as a native library too; the app cannot know where the system put
   * those. Unchanged for the other servers.
   */
  fun withNaive(context: Context, config: String): String {
    val json = JSONObject(config)
    val naive = json.optJSONObject("naive") ?: return config
    naive.put("binary", File(context.applicationInfo.nativeLibraryDir, "libnaive.so").absolutePath)
    return json.toString()
  }

  /** Where the engine's log lines go: the core's error log, which the app shows. */
  fun logFile(config: String, fallback: File): File =
    try {
      JSONObject(config).optJSONObject("log")?.optString("error")?.takeIf { it.isNotEmpty() }?.let(::File)
    } catch (e: Exception) {
      null
    } ?: fallback

  /**
   * The delay of one server: the engine fetches [url] through it itself and
   * prints `delay <ms>`; it serves nothing meanwhile, so no port is open.
   * Blocks for up to [timeoutSeconds] and then some.
   */
  fun probe(context: Context, mode: String, config: String, url: String, timeoutSeconds: Int): Long {
    val file = File(context.cacheDir, "probe-${java.util.UUID.randomUUID()}.json")
    file.writeText(withNaive(context, config))
    try {
      val process =
        ProcessBuilder(
            binary(context).absolutePath, mode, "--config", file.absolutePath,
            "--probe", url, "--timeout", "${timeoutSeconds}s",
          )
          .start()
      val done = CountDownLatch(1)
      // Read in the background, so a tunnel that never comes up cannot hang us.
      var delay = -1L
      thread(name = "engine-probe", isDaemon = true) {
        process.inputStream.bufferedReader().useLines { lines ->
          for (line in lines) {
            if (line.startsWith("delay ")) {
              delay = line.removePrefix("delay ").trim().toLongOrNull() ?: -1L
              break
            }
          }
        }
        done.countDown()
      }
      thread(name = "engine-probe-err", isDaemon = true) {
        // Drained, or a chatty engine would block on a full pipe.
        process.errorStream.use { stream ->
          val buffer = ByteArray(4096)
          while (stream.read(buffer) >= 0) {}
        }
      }
      if (!done.await(timeoutSeconds + 10L, TimeUnit.SECONDS)) process.destroy()
      return delay
    } finally {
      // The engine removes it once read; not if it failed to start.
      file.delete()
    }
  }

  /**
   * The engine of the VPN mode: a child of the service, as the app's user.
   * Xray runs inside the app, so the engine accepts the app's uid, which this
   * class vouches for through [OwnerServer].
   */
  class Child(
    private val context: Context,
    private val mode: String,
    config: String,
    /** Gets the engine's log lines, appended. */
    private val log: File,
    private val onExit: (String) -> Unit,
  ) {
    private val configFile = File(context.filesDir, "run/engine.json")
    private var config = JSONObject(withNaive(context, config))
    private var process: java.lang.Process? = null
    private val exited = CountDownLatch(1)
    private var owner: OwnerServer? = null
    @Volatile private var stopping = false

    /** Starts the engine and waits until it listens; returns why it could not. */
    fun start(): String? {
      val started =
        try {
          val socket = config.optString("ownerSocket")
          if (socket.isNotEmpty()) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
              owner = OwnerServer(context, socket.removePrefix("@")).also { it.start() }
            } else {
              // Before Android 10 /proc/net is readable to apps, and there is no
              // system call to ask instead; the engine reads it itself.
              config.remove("ownerSocket")
            }
          }
          configFile.parentFile?.mkdirs()
          configFile.writeText(config.toString())
          // The secrets are in the file; the engine removes it once read.
          config = JSONObject()

          ProcessBuilder(binary(context).absolutePath, mode, "--config", configFile.absolutePath).start()
        } catch (e: Exception) {
          stop()
          return "cannot start the engine: ${e.message}"
        }
      process = started
      // Its stdin stays open: the engine exits when it closes, with us.
      val ready = CountDownLatch(1)
      thread(name = "engine-out", isDaemon = true) {
        started.inputStream.bufferedReader().useLines { lines ->
          for (line in lines) if (line.startsWith("ready ")) ready.countDown()
        }
      }
      thread(name = "engine-log", isDaemon = true) {
        // By hand: ProcessBuilder.Redirect needs Android 8.
        java.io.FileOutputStream(log, true).use { out -> started.errorStream.copyTo(out) }
      }
      thread(name = "engine-wait", isDaemon = true) {
        val code = started.waitFor()
        exited.countDown()
        ready.countDown()
        if (!stopping) onExit("the engine stopped (exit code $code)")
      }
      if (!ready.await(READY_TIMEOUT_MS, TimeUnit.MILLISECONDS) || exited.count == 0L) {
        stop()
        return "the engine did not start; see the log"
      }
      return null
    }

    fun stop() {
      stopping = true
      process?.let { child ->
        try {
          child.outputStream.close()
        } catch (e: Exception) {
          // Gone already.
        }
        if (!exited.await(3, TimeUnit.SECONDS)) child.destroy()
      }
      process = null
      owner?.close()
      owner = null
      configFile.delete()
    }
  }

  /**
   * Answers the engine's "who owns this connection" (Android 10+ hides
   * /proc/net from apps; the VPN app may ask the system instead): one line,
   * `tcp <client> <server>` or `udp <client> <relay>`, answered with the
   * owner's uid or -1. The socket's name is random per run and only the
   * engine knows it; connections from other uids get no answer anyway.
   */
  class OwnerServer(context: Context, name: String) {
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)
    private val server = LocalServerSocket(name)
    @Volatile private var closed = false

    fun start() {
      thread(name = "engine-owner", isDaemon = true) {
        while (!closed) {
          val client =
            try {
              server.accept()
            } catch (e: Exception) {
              break
            }
          try {
            client.use { answer(it) }
          } catch (e: Exception) {
            Log.w(TAG, "owner question failed", e)
          }
        }
      }
    }

    private fun answer(client: LocalSocket) {
      if (client.peerCredentials.uid != Process.myUid()) return
      client.soTimeout = 2000
      val question = client.inputStream.bufferedReader().readLine()?.trim().orEmpty()
      client.outputStream.write("${owner(question)}\n".toByteArray())
    }

    private fun owner(question: String): Int {
      if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return -1
      val parts = question.split(" ")
      if (parts.size != 3) return -1
      val protocol =
        when (parts[0]) {
          "tcp" -> OsConstants.IPPROTO_TCP
          "udp" -> OsConstants.IPPROTO_UDP
          else -> return -1
        }
      val local = address(parts[1]) ?: return -1
      val remote = address(parts[2]) ?: return -1
      return try {
        val uid = connectivity.getConnectionOwnerUid(protocol, local, remote)
        // An unconnected UDP socket is known by its local end alone.
        if (uid == Process.INVALID_UID && protocol == OsConstants.IPPROTO_UDP) {
          connectivity.getConnectionOwnerUid(protocol, local, InetSocketAddress(InetAddress.getByName("0.0.0.0"), 0))
        } else {
          uid
        }
      } catch (e: Exception) {
        Log.w(TAG, "getConnectionOwnerUid failed", e)
        -1
      }
    }

    /** `1.2.3.4:5` or `[::1]:5`, numeric only. */
    private fun address(text: String): InetSocketAddress? {
      val colon = text.lastIndexOf(':')
      if (colon <= 0) return null
      val host = text.substring(0, colon).removePrefix("[").removeSuffix("]")
      val port = text.substring(colon + 1).toIntOrNull() ?: return null
      if (host.isEmpty() || !host.all { it.isDigit() || it == '.' || it == ':' || it in 'a'..'f' || it in 'A'..'F' }) {
        return null
      }
      return InetSocketAddress(InetAddress.getByName(host), port)
    }

    fun close() {
      closed = true
      // Closing alone does not wake a thread blocked in accept().
      try {
        Os.shutdown(server.fileDescriptor, OsConstants.SHUT_RDWR)
      } catch (e: Exception) {
        // Already gone.
      }
      try {
        server.close()
      } catch (e: Exception) {
        // Already gone.
      }
    }
  }
}
