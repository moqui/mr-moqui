# Java HTML dropdown parity

The HTML renderer uses the Java reference's bundled Select2 4.0.3 and Bootstrap theme for drop-down widgets, including static dropdowns. Native selects remain the submitted value source. Menus mount inside native dialogs so they remain interactive above the modal top layer. Native change events also update dependent dropdowns.

Widget width comes from its rendered native bounds: jQuery outerWidth double-counts intrinsic select padding in Chromium. The Java baseline margin has enough specificity to survive asynchronously loaded theme CSS.

## Verification

Nineteen checks pass: eight macro checks, eight client helper checks, CSS layout, dialog browser behavior, and the new Select2 browser regression. The latter uses the actual bundled libraries and verifies menu visibility, selection, submitted values, native change propagation, dependent option reload, width, and baseline spacing.

UI proof is in the companion regression checkout at `.runs/ui-narrated-20261024/select-geometry-fix/watch.html`. Its 22 recorded UI steps use fresh independent demo databases and the same Java click coordinates on both engines. Each step retains raw browser frames, screenshots, comparison JSON and Piper narration.

At step 00004 all six customer comboboxes have identical bounds on Java and Rust. Role: (482.328125, 100.703125), 222 by 34. Customer class: (482.328125, 147.703125), 201 by 34. First name: (482.328125, 258.703125), 288 by 34. Step 00005 records the opened role menus.

This is runtime-asset verification using the existing Rust executable, not a fresh source build. `runtime.json` records artifact hashes, source diff and frozen asset hashes. The run remains failed overall because screen pixels and widget metadata still differ, including required-field colors and the surrounding navigation. No comparison masks are used. These interactions do not establish full business workflow completion or qapps parity.
