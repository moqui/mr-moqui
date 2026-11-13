# Screen extensions of @component@

A file here at the path of an app's screen extends that screen: `example/screen/Gallery/ExampleDetail.xml` (the path
after `component://`) or `Gallery/ExampleDetail.xml` (the path under the app's `screen` directory). Its root is
`<screen-extend>`; a `form-single` or `form-list` of the base screen's name adds fields (`after="otherField"` places
one), and `widgets-extend name="Anchor" where="before|after|prepend|append"` adds widgets at a named element.
