# HTML dialogs: Java reference

The paired browser run opens the same rendered control at the Java control's center on both engines, with a 1440×1000 viewport. Test data is disposable demo data; all application mutations use the screens. Each step retains screenshots, actual Chromium screencast frames, hit targets, widget inventories, coordinate comparison, pixel difference and a Piper-narrated side-by-side MP4. Screen differences are not masked.

This change fixes native HTML dialog placement, Java header/body spacing, the close button position, and grouped field gutters. Clicking padding inside the native dialog no longer dismisses it; clicking its backdrop does. Desktop top spacing is 30px and narrow-screen top spacing is 10px.

Observed New Person first-name input before the fix: Java (482.328, 102.703), Rust (481.656, 446.984). With this change both engines render it at (482.328, 102.703), 288×34. The last-name field also matches exactly at (482.328, 151.703).

In New Person Customer, Rust previously wrapped the name fields onto separate rows. With the grouped-field gutter fix, first/last names stay on one row; their x coordinates differ from Java by 0px/0.110px. The remaining 4px y offset comes from preceding dropdowns, which use a different widget and are outside this fix. Whole-screen parity and complete business workflows are **not** claimed.

Local evidence in the companion java-regression-test checkout:

- `.runs/ui-narrated-20261024/paired-screens/steps/00010/step.json`: original first-name position mismatch.
- `.runs/ui-narrated-20261024/dialog-fix/steps/00012/step.json` and `videos/step-00012.mp4`: identical New Person first-name bounds after dialog placement fix.
- `.runs/ui-narrated-20261024/form-layout-fix/steps/00013/step.json` and `videos/step-00013.mp4`: customer name fields share a row after gutter correction; remaining dropdown offset is still flagged.
- Each run's `runtime.json` records artifact hashes; later runs retain frozen Rust runtime assets and their hashes. Runtime CSS/JS changes do not require rebuilding the Rust executable. The UI run uses the existing executable with the changed runtime assets; a fresh executable build is not claimed.

Validation: browser regression covers desktop/narrow dialog top spacing, horizontal centering, header dimensions, focus, close-button alignment, customer name wrapping and inside/outside click behavior. Existing HTML macro, client and CSS metric tests also run. The CSS test fixture now uses a local HTML file and Chromium's temporary profile, avoiding persistent-profile CLI hangs in the test environment.
