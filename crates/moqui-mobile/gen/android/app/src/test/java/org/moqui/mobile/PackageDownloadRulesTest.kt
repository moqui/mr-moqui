// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import androidx.work.NetworkType
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** plans/Maps-Offline.md §2.4: the offline map download's constraints and its resume. */
class PackageDownloadRulesTest {
  @Test
  fun aDownloadWaitsForANetworkWiFiWhenAskedAndStorageNotLow() {
    val any = PackageDownloadRules.constraints(wifiOnly = false)
    assertEquals(NetworkType.CONNECTED, any.requiredNetworkType)
    assertTrue(any.requiresStorageNotLow())
    val wifi = PackageDownloadRules.constraints(wifiOnly = true)
    assertEquals(NetworkType.UNMETERED, wifi.requiredNetworkType)
    assertTrue(wifi.requiresStorageNotLow())
  }

  @Test
  fun aCutOrAStopIsResumedAndAnEndIsDone() {
    // a cut or a server error: tried again (Rust kept the part and asks from its length)
    assertEquals(PackageDownloadRules.Outcome.RETRY, PackageDownloadRules.outcome(PackageDownloadRules.RETRY, 0, false))
    // stopped by WorkManager (Wi-Fi gone, ten minutes up): run again, whatever it answered
    assertEquals(PackageDownloadRules.Outcome.RETRY, PackageDownloadRules.outcome(PackageDownloadRules.DONE, 3, true))
    assertEquals(PackageDownloadRules.Outcome.RETRY, PackageDownloadRules.outcome(PackageDownloadRules.RETRY, 50, true))
    // installed, refused, cancelled or stale: done
    assertEquals(PackageDownloadRules.Outcome.SUCCESS, PackageDownloadRules.outcome(PackageDownloadRules.DONE, 0, false))
    // a download that keeps failing is given up at last
    val last = PackageDownloadRules.MAX_ATTEMPTS - 1
    assertEquals(PackageDownloadRules.Outcome.RETRY, PackageDownloadRules.outcome(PackageDownloadRules.RETRY, last - 1, false))
    assertEquals(PackageDownloadRules.Outcome.FAILURE, PackageDownloadRules.outcome(PackageDownloadRules.RETRY, last, false))
  }

  @Test
  fun oneWorkPerAccountAndPackage() {
    assertEquals(
      PackageDownloadRules.uniqueName("a.example.com-1", "P1"),
      PackageDownloadRules.uniqueName("a.example.com-1", "P1"),
    )
    assertNotEquals(
      PackageDownloadRules.uniqueName("a.example.com-1", "P1"),
      PackageDownloadRules.uniqueName("b.example.com-1", "P1"),
    )
    assertTrue(PackageDownloadRules.accountTag("a.example.com-1").endsWith("a.example.com-1"))
  }
}
