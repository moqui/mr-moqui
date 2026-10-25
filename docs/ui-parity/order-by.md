# HTML Order By widget

Find Options now uses the Java runtime’s Selectivity widget and bundled assets. It preserves initial sorting, ascending/descending choices, selected chips, removals, hidden form values and the reference 300-pixel dropdown. Orderable header fields remain available even when excluded from displayed columns. Generated widget inputs do not steal initial dialog focus.

Validation: 9 macro, 8 client, one Order By browser, one CSS browser and one dialog browser check pass (20 checks). The Order By browser check verifies selection and removal update the submitted hidden value, dropdown width, 32-pixel search-input height, and opening inside a modal.

Actual paired UI evidence is in java-regression-test `.runs/ui-narrated-20261024/order-by-layout-fix`: 11 steps with 11 Piper-narrated paired screencast clips. Steps 5–6 open the same nine choices and select Period Name (Asc) using the same coordinates in both engines, without a recovery click. Both show From Date (Desc) and Period Name (Asc) chips afterward. Search-input x/width/height and option x/width/height match; Rust is vertically offset by 2.15625 pixels. Both engines shut down without forced kill or missing markers (`.runs/2026-10-25_06-53-34.486488Z/shutdown.json`).

Limits: full-screen parity still fails. The recording exposes a missing Rust Find submit button, additional form differences and calendar offsets. These remain separate work; this run does not establish a successful Rust Find submission or a completed business workflow. Runtime proof uses the existing Rust binary with frozen changed assets, not a fresh Rust source build. The earlier `order-by-widget` run retains the pre-CSS-fix geometry failure.
