# qapps inline label spacing

Vue 3 condenses a newline between an inline label and the following text component, removing the space preserved by Java's Vue 2 renderer. Emit an explicit space after labels in both merged and static QVT macros. Pass undefined for empty text-line labels so Quasar 2 does not apply labeled-input padding.

Validation: 15 QVT macro and 22 Vue component checks pass (37). Actual run: java-regression-test `.runs/ui-narrated-20261024/qapps-customer-inline-label-spacing`, eight paired steps and eight Piper-narrated clips.

All 13 named dialog controls have exactly matching x/y/width/height at step 4, including country code, area code and phone number. Both name-entry clicks and Create use shared coordinates without recovery. Both engines create UI / Proof6 and retain both names after browser reload.

Limits: full-screen parity still fails; shell, field focus-border appearance and other widgets differ. Rust shutdown passes; Java exits without force but fails shutdown-marker verification (missing ArtifactHitBins stored and Moqui ExecutionContextFactory Destroyed), retained in `.runs/2026-10-25_07-46-52.636114Z/shutdown.json`. Existing binary plus frozen assets, not a fresh Rust source build. Customer creation/name persistence only, not complete customer-process coverage.
