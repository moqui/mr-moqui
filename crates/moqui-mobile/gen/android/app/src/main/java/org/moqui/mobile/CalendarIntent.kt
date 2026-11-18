// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.provider.CalendarContract
import android.util.Log

/**
 * A chat event added to the phone's calendar (plans/Comms-Messaging.md §18.3 phase 19): the calendar app opened on a new
 * event filled in (`ACTION_INSERT`), which the person saves there. The app reads and writes no calendar itself, so it
 * needs no calendar permission. The page asks through `moquiDesktop.calendarInsert` (the `calendar_insert` command).
 */
object CalendarIntent {
    @JvmStatic
    fun insert(context: Context, title: String, begin: Long, end: Long, allDay: Boolean, zone: String, place: String, description: String): Boolean {
        val intent = Intent(Intent.ACTION_INSERT).setData(CalendarContract.Events.CONTENT_URI)
            .putExtra(CalendarContract.Events.TITLE, title)
            .putExtra(CalendarContract.EXTRA_EVENT_BEGIN_TIME, begin)
            .putExtra(CalendarContract.EXTRA_EVENT_END_TIME, end)
            .putExtra(CalendarContract.EXTRA_EVENT_ALL_DAY, allDay)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (zone.isNotEmpty()) intent.putExtra(CalendarContract.Events.EVENT_TIMEZONE, zone)
        if (place.isNotEmpty()) intent.putExtra(CalendarContract.Events.EVENT_LOCATION, place)
        if (description.isNotEmpty()) intent.putExtra(CalendarContract.Events.DESCRIPTION, description)
        return try {
            context.startActivity(intent)
            true
        } catch (err: ActivityNotFoundException) {
            Log.i("MoquiCalendar", "No calendar app takes a new event")
            false
        } catch (err: RuntimeException) {
            Log.w("MoquiCalendar", "Could not open the calendar app", err)
            false
        }
    }
}
