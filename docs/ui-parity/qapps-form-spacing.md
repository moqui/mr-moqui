# qapps grouped form spacing

Merged and server-static QVT forms use Java’s eight-pixel horizontal margins and automatic vertical margins for grouped fields and row titles. Remove the extra flex gap and restore eight-pixel card-section padding.

Validation: 15 macro and 25 static client checks pass, plus two CSS browser checks (42 total). Computed CSS checks cover card padding, the 16-pixel gap between grouped fields and vertical alignment. The CSS launcher uses Chromium’s temporary profile because explicit profile directories hang in this environment.

UI evidence: java-regression-test `.runs/ui-narrated-20261024/qapps-customer-form-spacing`, eight paired steps with Piper narration. First/last-name x positions and dimensions now match Java exactly (422/695 pixels, width 233, height 40), as do email and Create x/width. Both create UI / Proof6 and retain names after reload. Both engines shut down without forced kill or missing markers (`.runs/2026-10-25_07-27-36.489145Z/shutdown.json`).

Limits: vertical offsets remain: names are two pixels above Java, email eight, and Create four. Dense-field margins, floating labels and dialog headers remain separate work; full-screen parity fails. Existing binary plus frozen changed assets, not a fresh Rust source build.
