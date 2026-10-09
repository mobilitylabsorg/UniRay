package dev.uniray.app

import android.content.Context
import android.content.pm.PackageManager
import android.util.Log
import com.topjohnwu.superuser.Shell
import java.lang.reflect.InvocationTargetException
import rikka.shizuku.Shizuku
import rikka.shizuku.ShizukuProvider

/** What elevated access the device offers. */
enum class Privilege {
  /** Nothing: the app runs with its own, ordinary permissions. */
  NONE,
  /** Shizuku, but started from ADB (uid 2000). No CAP_NET_ADMIN: conveniences
   * only, never stealth. */
  SHIZUKU_SHELL,
  /** Shizuku started as root (uid 0): stealth-capable. */
  SHIZUKU_ROOT,
  /** Real root (Magisk/KernelSU/su): stealth-capable. */
  ROOT;

  val isRoot
    get() = this == ROOT || this == SHIZUKU_ROOT

  /** Stealth needs CAP_NET_ADMIN (tproxy, a root TUN), which only real root
   * gives; an ADB-shell Shizuku cannot. */
  val canStealth
    get() = isRoot
}

/** The detected privilege plus what the UI needs to offer the next step. */
data class PrivilegeState(
  val level: Privilege,
  /** Shizuku's binder is alive (the manager app is running). */
  val shizukuAvailable: Boolean,
  /** Shizuku is bound and has granted us permission. */
  val shizukuGranted: Boolean,
  /** Whether the kernel and iptables support the TPROXY target. */
  val hasTproxy: Boolean = false,
)

class ShellResult(val code: Int, val out: List<String>, val err: List<String>) {
  val ok
    get() = code == 0

  fun outText() = out.joinToString("\n")

  fun errText() = err.joinToString("\n")
}

/**
 * One way to run commands with elevated privileges, whichever the device has.
 *
 * Real root (Magisk/KernelSU/su) is the strong path: a long-lived root shell
 * through libsu. Shizuku is the alternative - root too when it was started as
 * root, otherwise an ADB shell (uid 2000) that cannot grant CAP_NET_ADMIN and
 * so cannot do stealth, only the conveniences.
 *
 * [detect] never prompts; it is safe to call from the UI. Actually taking root
 * ([execRoot]) may pop the superuser grant dialog the first time.
 */
object PrivilegedShell {
  private const val TAG = "UniRayPriv"
  const val SHIZUKU_PERMISSION_REQUEST = 4001
  @Volatile private var cachedTproxy: Boolean? = null

  init {
    // Create shells on demand with a short timeout; never keep a cached root
    // shell alive longer than a request needs it.
    Shell.enableVerboseLogging = false
    Shell.setDefaultBuilder(Shell.Builder.create().setTimeout(15))
  }

  /** Current privilege without prompting, for display and routing decisions. */
  fun detect(context: Context): PrivilegeState {
    val shizukuAvailable = shizukuAlive()
    val shizukuGranted = shizukuAvailable && shizukuPermissionGranted()
    val shizuku = if (shizukuGranted) shizukuLevel() else Privilege.NONE
    val level =
      when {
        // Already-granted real root is the strongest and surest.
        Shell.isAppGrantedRoot() == true -> Privilege.ROOT
        shizuku != Privilege.NONE -> shizuku
        else -> Privilege.NONE
      }
    val hasTproxy = if (level.canStealth) checkTproxy(level) else false
    return PrivilegeState(level, shizukuAvailable, shizukuGranted, hasTproxy)
  }

  /** Tests whether the kernel has the iptables TPROXY target. */
  fun checkTproxy(level: Privilege): Boolean {
    if (!level.canStealth) return false
    cachedTproxy?.let { return it }
    val script =
      "(iptables -w -t mangle -F UNIRAY_CHK 2>/dev/null; iptables -w -t mangle -X UNIRAY_CHK 2>/dev/null); if iptables -w -t mangle -N UNIRAY_CHK 2>/dev/null && iptables -w -t mangle -A UNIRAY_CHK -p tcp -j TPROXY --on-ip 127.0.0.1 --on-port 12345 --tproxy-mark 0x1/0x1 2>/dev/null; then echo 1; else echo 0; fi; iptables -w -t mangle -F UNIRAY_CHK 2>/dev/null; iptables -w -t mangle -X UNIRAY_CHK 2>/dev/null"
    val res = execPrivileged(level, script)
    val supported = (res.ok || res.code == 0) && res.outText().contains("1")
    Log.i(TAG, "checkTproxy: ok=${res.ok}, code=${res.code}, out=${res.outText()}, err=${res.errText()}, supported=$supported")
    if (supported) {
      cachedTproxy = true
    }
    return supported
  }

  /** Whether real root is available, prompting for the grant if it must. Call
   * off the main thread. */
  fun requestRoot(): Boolean {
    cachedTproxy = null
    return try {
      Shell.getShell().isRoot
    } catch (e: Throwable) {
      Log.w(TAG, "root shell failed", e)
      false
    }
  }

  /** Runs [script] with `sh` as root (through libsu). Call off the main thread. */
  fun execRoot(script: String): ShellResult {
    return try {
      val result = Shell.cmd(script).exec()
      ShellResult(result.code, result.out, result.err)
    } catch (e: Throwable) {
      Log.w(TAG, "root exec failed", e)
      ShellResult(-1, emptyList(), listOf(e.message ?: e.toString()))
    }
  }

  /**
   * Runs [script] through Shizuku (as root or ADB shell, per how Shizuku was
   * started). Best-effort: Shizuku's process API is a hidden method reached by
   * reflection, so this degrades to a failure rather than failing to compile.
   */
  fun execShizuku(script: String): ShellResult {
    if (!shizukuAlive()) {
      return ShellResult(-1, emptyList(), listOf("Shizuku is not reachable from this process"))
    }
    return try {
      val newProcess =
        Shizuku::class
          .java
          .getDeclaredMethod("newProcess", Array<String>::class.java, Array<String>::class.java, String::class.java)
          .apply { isAccessible = true }
      val process =
        newProcess.invoke(null, arrayOf("sh", "-c", script), null, null) as java.lang.Process
      var err = emptyList<String>()
      val errThread = kotlin.concurrent.thread(name = "shizuku-err") {
        try {
          err = process.errorStream.bufferedReader().readLines()
        } catch (e: Exception) {
          // Stream closed
        }
      }
      val out = try {
        process.inputStream.bufferedReader().readLines()
      } catch (e: Exception) {
        emptyList()
      }
      var code = -1
      val waitThread = kotlin.concurrent.thread(name = "shizuku-wait") {
        try {
          code = process.waitFor()
        } catch (e: Throwable) {
          code = -1
        }
      }
      waitThread.join(15_000)
      if (waitThread.isAlive) {
        try {
          process.destroy()
        } catch (e: Throwable) {
          // Process gone
        }
      }
      ShellResult(code, out, err)
    } catch (e: Throwable) {
      // Reflection wraps whatever Shizuku threw; report that, not the wrapper.
      val cause = (e as? InvocationTargetException)?.targetException ?: e
      Log.w(TAG, "shizuku exec failed", cause)
      ShellResult(-1, emptyList(), listOf(cause.message ?: cause.toString()))
    }
  }

  /**
   * Shizuku hands its binder only to the process hosting [ShizukuProvider]
   * (the main one). Services in another process (`:vpn`) fetch it from there
   * before any Shizuku call. A no-op when the binder is already here.
   */
  fun ensureShizukuBinder(context: Context) {
    if (shizukuAlive()) return
    try {
      ShizukuProvider.requestBinderForNonProviderProcess(context)
    } catch (e: Throwable) {
      Log.w(TAG, "fetching the Shizuku binder failed", e)
    }
  }

  /** Runs a privileged [script], preferring real root, else Shizuku-root. */
  fun execPrivileged(level: Privilege, script: String): ShellResult =
    when (level) {
      Privilege.ROOT -> execRoot(script)
      Privilege.SHIZUKU_ROOT -> execShizuku(script)
      // An ADB shell cannot do privileged network work; refuse clearly.
      else -> ShellResult(-1, emptyList(), listOf("no root privilege for: $script"))
    }

  private fun shizukuAlive(): Boolean =
    try {
      Shizuku.pingBinder() && !Shizuku.isPreV11()
    } catch (e: Throwable) {
      false
    }

  private fun shizukuPermissionGranted(): Boolean =
    try {
      Shizuku.checkSelfPermission() == PackageManager.PERMISSION_GRANTED
    } catch (e: Throwable) {
      false
    }

  /** Shizuku's own uid decides what it can do: 0 is root, 2000 is an ADB shell. */
  private fun shizukuLevel(): Privilege =
    try {
      if (Shizuku.getUid() == 0) Privilege.SHIZUKU_ROOT else Privilege.SHIZUKU_SHELL
    } catch (e: Throwable) {
      Privilege.NONE
    }
}
