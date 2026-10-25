# qapps dense fields and floating labels

Restore Java's six-pixel dense-field margins, labels over the outline, eight-pixel input padding, focused input underline and error-only bottom space. Keep nested and row fields free of duplicate margins. Use Quasar 2's primary-color variable for the underline.

Validation: two computed CSS browser checks and 22 Vue component checks pass. The CSS fixture checks dense and row margins, label position and padding, and input padding.

UI evidence: java-regression-test `.runs/ui-narrated-20261024/qapps-customer-field-labels`, eight paired browser steps and eight Piper-narrated clips. Both create UI / Proof6 and retain both names after reload. Both shutdown verifications pass (`.runs/2026-10-25_07-32-29.896587Z/shutdown.json`).

Limits: the initial capture can observe an animating dialog or a loading state dropdown, so it does not prove final geometry or option labels. A bounded transition/control-stability wait was added to the recorder for a fresh run. Full-screen comparison fails and this is not complete customer-process coverage. Uses the existing binary plus frozen changed assets, not a fresh Rust source build.
