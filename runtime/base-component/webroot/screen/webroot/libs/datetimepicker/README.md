# Java calendar assets

The datetimepicker 4.17.47 source is copied unchanged from Java's webroot/js/bootstrap-datetimepicker, with its MIT notice retained. Moment with locales, Bootstrap's CSS and Glyphicons fonts come from that same reference webroot. Bootstrap CSS is wrapped in CSS @scope for `.bootstrap-datetimepicker-widget` so its styles apply inside the calendar popup. This requires a browser supporting CSS @scope, alongside the renderer's existing native dialog requirement. The Bootstrap `.dropdown-menu` rule is also applied with `:where(:scope)` at low specificity so the plugin can override its padding and display because an implicit scoped selector does not match the scope root. The other library contents are unchanged.

The calendar plugin's own fallback toggles date/time views when Bootstrap's collapse JavaScript is absent; the full Bootstrap JavaScript is not loaded.
