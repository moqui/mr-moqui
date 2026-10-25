# Java HTML date ranges

Date-find From/Thru fields now use the same Java calendar as date-time fields. Both bounds retain their original parameter names and default values. The macro shares input markup, applies Java fixed date-find sizes/maximum lengths and five-minute stepping, and uses the two inline `form-date-find` wrappers with translated From/Thru labels. Time-only filters retain Java text-pattern validation.

Validation: macro checks, the CSS layout browser check and the calendar interaction browser check pass. The calendar browser check selects a day, verifies native form data/change notification, and switches to a time view and increments five minutes.

UI-only evidence: java-regression-test `.runs/ui-narrated-20261024/time-period-date-range` retains the initial Time Period Find Options sweep. Its earlier Type dropdown stays open over Rust's calendar target; this is recorded as a shared-coordinate mismatch and is not a passing popup test. `.runs/ui-narrated-20261024/date-range-popups` repeats Find Options with only calendar children to inspect the range widgets without that prior dropdown interaction. Consult step JSON and the Piper videos manifest for the final result.

Limits: prior widgets in Find Options still affect vertical positions. Whole-screen parity and full period-closing workflow success remain unestablished. Runtime verification uses the existing Rust executable plus frozen changed runtime assets; no fresh Rust build is claimed.
