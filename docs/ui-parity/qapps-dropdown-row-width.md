# qapps inline dropdown widths

Java's Quasar 1 select renders an empty append ahead of the arrow, so its arrow uses two pixels of left padding. Quasar 2 omits that append and uses six pixels. The extra four pixels on each Country/State dropdown wrap State to a second row, adding 52 pixels to a centered dialog. Restore the two-pixel arrow padding and pass undefined for an empty dropdown label so Quasar 2 does not apply labeled-field padding.

Validation: 22 Vue component checks and two CSS browser checks pass, including computed arrow padding.

UI evidence: java-regression-test `.runs/ui-narrated-20261024/qapps-customer-dropdown-row-width`, eight paired steps and eight Piper-narrated clips. Both create UI / Proof6 and reload both names. Both shutdown checks pass (`.runs/2026-10-25_07-42-56.882352Z/shutdown.json`).

At step 4, name, email, address, city, Country/State, Create and close bounds match Java exactly; State no longer wraps. Name entry and Create clicks use shared coordinates without recovery. Phone input x offsets remain -3/-6/-9 pixels; global shell and other widgets still differ, so the full-screen comparison correctly exits 1. Existing Rust binary plus frozen changed assets; no fresh source-build or complete workflow claim.
