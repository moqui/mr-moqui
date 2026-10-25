# HTML Find Options fields and layout

Find Options renders every filterable header field in declaration order, including header-only submit buttons omitted from displayed table columns. Saved Finds and Find Parameters headings follow the Java conditions. Clear Parameters uses the reference small primary button; inline Select2 search fields retain their widget dimensions.

Validation: 10 macro and 8 client checks, plus Select2, dialog, Order By and CSS browser checks pass (22 total). Regression coverage includes off-column filters and submit buttons, heading spacing, clear-button height and multiple Select2 height.

Paired UI evidence in java-regression-test `.runs/ui-narrated-20261024/find-dialog-submit-verified` has 10 steps and 10 Piper-narrated screencast clips. Both engines open Find Options, choose Period Name (Asc), open the date calendars, click Find, then reload the results. Shared-coordinate clicks for opening, sorting and submitting hit the intended controls without recovery. Both retain `-fromDate,periodName` in the reloaded UI, show the Time Period list and close the dialog. All observed Find controls align within 0.016 pixels in the preceding `find-dialog-layout-verified` capture. Both engines shut down without forced kill or missing markers (`.runs/2026-10-25_07-16-38.035646Z/shutdown.json`).

Limits: full-screen parity remains failed; the overall runner exits nonzero for those differences. This proves Find submission and reloaded sorting, not a complete accounting workflow. The earlier layout capture retains a failed verification caused by checking URL query parameters for a POST; the fresh run verifies the hidden field in the visible results UI. Runtime proof uses the existing Rust binary with frozen changed assets, not a fresh source build.
