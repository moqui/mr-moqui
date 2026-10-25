# Java HTML calendar widget

Date-time fields use Java's bundled Bootstrap datetimepicker 4.17.47 and Moment assets instead of native browser date inputs. Time-only fields retain Java's text pattern. Input sizes, maximum lengths, formats, calendar button, top toolbar and five-minute default match the reference macro. Picker changes update the native form field and dispatch a change from its wrapper for dependent widgets.

Bootstrap styles are scoped to the calendar. Its root dropdown rule explicitly uses `:scope`; without that rule the popup would enter document flow and move the calendar button. The browser regression checks floating placement and a stable button, selecting a day, native form data, change notification, closing after selection, switching to time and incrementing five minutes.

Validation: 20 checks pass (8 macro, 8 client, CSS layout, dialog browser, Select2 browser, calendar browser). Chromium browser checks run sequentially.

Paired UI evidence is collected by the java-regression-test harness under `.runs/ui-narrated-20261024/`. `calendar-widget-verified` retains the failed in-flow popup iteration, including 24 narrated browser steps. `calendar-popup-fix` repeats the customer create/reload scenario and actual Birth Date calendar click after the root rule correction. Consult its step JSON and runtime manifest for final results and frozen asset hashes.

Limits: date-find range fields still use the existing renderer and need a separate change. Whole-screen parity remains unmet; this patch does not resolve navigation, panel or general focus styling. Runtime verification uses the existing Rust executable with frozen changed runtime assets, not a fresh Rust source build. These customer interactions do not establish all business workflow coverage.
