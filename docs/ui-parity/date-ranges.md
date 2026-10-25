# Java HTML date ranges

Date-find From/Thru fields now use the same Java calendar as date-time fields. Both bounds retain their original parameter names and default values. The macro shares input markup, applies Java fixed date-find sizes/maximum lengths and five-minute stepping, and uses the two inline `form-date-find` wrappers with translated From/Thru labels. Time-only filters retain Java text-pattern validation. Initial dialog focus skips generated Tom Select/Select2 search inputs so opening Find Options does not cover the calendars with an unrelated dropdown.

Validation: macro checks, the CSS layout browser check and the calendar interaction browser check pass. The calendar browser check selects a day, verifies native form data/change notification, and switches to a time view and increments five minutes.

UI-only evidence: java-regression-test `.runs/ui-narrated-20261024/date-range-hit-fix` retains seven paired UI steps with Piper-narrated screencasts. Steps 5 and 6 open From and Thru calendars on both engines at the Java coordinates without recovery clicks. Both popup texts match. Input and calendar-button widths/horizontal positions match; Rust is 2.15625 pixels lower because of earlier controls. Both engine shutdown checks pass in `.runs/2026-10-25_06-39-36.727332Z/shutdown.json`.

Earlier `time-period-date-range`, `date-range-popups` and `date-range-focus-fix` runs remain intact. The first two exposed an unrelated Order By dropdown over the target. The last also exposed a recorder mistake: it clicked an already-open Rust calendar again because a span without a name was misidentified as a missed target. The recorder correction is in the harness; the fresh run above avoids that second click. These earlier final screenshots are not passing popup evidence.

Limits: prior widgets in Find Options still affect vertical positions. Whole-screen parity and full period-closing workflow success remain unestablished. Runtime verification uses the existing Rust executable plus frozen changed runtime assets; no fresh Rust build is claimed.
