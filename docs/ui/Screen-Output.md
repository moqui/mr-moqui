# Screen output

How a screen's response reaches the client: kept whole, or sent while the render writes it, with the rows of a long
list read as they are written. The code is `crates/moqui-core/src/web/stream.rs` (the sink and the commit),
`crates/moqui-core/src/screen/mod.rs` (the request's head hook), `crates/moqui-host/src/http/stream.rs` (the
connection's side), and for the rows `crates/moqui-core/src/script/host/cursor.rs` and `screen/sri/forms.rs`.

## Buffer, then stream

A response is kept in memory until its body reaches the `screen-facade` `stream-threshold` (65536 bytes by default).
Then it commits, as a servlet response does when its buffer fills:

- **The head goes first.** The status, the headers set so far, the session cookie and token, and the conf's
  `screen-render` headers and CORS go to the client, with the body written so far.
- **Then the rest as it comes.** Each later write goes to the connection, in chunks of about 32 KiB. A compressed
  response streams too: the compressor flushes whenever the body waits for more.
- **A smaller page is sent whole**, with a `Content-Length`, its error page and its 304. `stream-threshold="0"` keeps
  every response whole.

What streams:

- **An html or text render** (html, html-print, text, csv, xml) of a request's own screen.
- **A web script's output**, when it is past the threshold or calls `flush()` / `flushBuffer()` on its writer.

What is kept whole:

- **The qvt family and tui**: what they write is rewound when a section fails, and the tui document is wrapped at the
  end.
- **xlsx and `/fop` PDFs**: they are made from the whole output.
- **A screen whose path has a servlet filter**: the filter sees the whole response.
- **A response that names its version** (an `ETag`), so it can still be answered 304.
- **A render inside another**: an email, a `makeRender`, an error page.

## After the head

- **Headers set late are dropped.** A header, cookie or status set after the commit is dropped with a warning, as a
  servlet's is once it commits. A user preference set after it is not sent.
- **The session is decided at the commit.** Whether to keep the request's session is decided when the head goes, from
  the head as it is then (a public `Cache-Control`, `declineSession()`).
- **A failure ends the body abnormally.** A render error, or a commit of the render's transaction that fails, after the
  head is written as `Error rendering page: ...` where the page stopped. The body then ends abnormally: HTTP/1.1 sends
  no last chunk, and HTTP/2 resets the stream. The client never takes the part it has for the whole page. A failed
  section is written where it was, as on any page.
- **A client that goes away stops the render.** Writes fail and the render writes nothing more. A client that takes
  nothing for the listener's `write-timeout` (60 s) counts as gone.
- **The worker stays held.** A streamed page holds its request worker, and the render's transaction, while the client
  reads. A small page frees them as soon as it is rendered.
- **The request deadline covers the head.** `request-timeout` covers the request up to its head. The render after the
  head is bounded by the script deadline and the screen's transaction timeout.

## Rows

Streaming the output bounds the response's memory; the rows of a long list are bounded by reading them as they are
written:

- **A csv, text or xml form-list** is read a row at a time: its `entity-find` opens the find's iterator
  (`EntityFind.iterator()`), and each row's `row-actions` and field `from` run when the row is written. A csv export
  of every row (`pageNoLimit`, the list's csv link) holds one row at a time. A form-list over a list from the screen's
  actions reads the list, but makes its rows as it writes them.
- **Kept whole:** an html, qvt or tui form-list, which reads its rows more than once (row forms, then rows), an
  aggregated one (`group-by`, `sub-list`, `show-total`), and xlsx, which sizes its columns to their text.
- **`use-iterator`** in an `entity-find` makes the list a cursor for the screen's own actions too: `iterate` and an
  unpaginated `section-iterate` read it a row at a time. A page with a limit is still a list. A form-list's row
  count (`_formListResultCount`) is known once its last row is written.
- **What an iterator allows.** `iterator()` is Java's `EntityListIterator` over a database cursor. As in Java it reads
  forward only, holding a row or two, unless the find's `resultSetType` is 1004 or 1005 (or it is for update): then it
  moves back and jumps, keeping the rows it has read. It reads in the transaction it opened in, beginning one when
  none is in place. A lookup for each row runs beside it, and so does a `force-new` transaction, on another
  connection. While it is open its connection opens no other cursor. On MySQL, whose open result takes no other
  query, the rows are read into memory instead. It closes after its last row; call `close()` when stopping early.
  A list from `list()` is only a list.

## Templates

A screen's `text` element (a `.fancy` file or `<text template="true">`) writes its output as it renders
(`sri.renderText`, `sri.writeInlineText`). A template's text and a screen it renders keep their order:
`<div>${sri.renderSubscreen()}</div>` writes the `div`, then the subscreen, then `</div>`. An interpolation that names
`sri.` runs between the text before it and the text after it. Rendered for its value (`sri.renderTextValue`,
`sri.renderInlineText`, a template from a script), a template returns its output as before.

The expression of a `${}`, and of an `<#if>`, `<#elseif>`, `<#list>` or `<#assign>` tag, is JavaScript, and the
template reads it as JavaScript to find its end. Brackets, strings (with their escapes, as `'it\'s'`), template
literals, `//` comments (to the end of the line) and `/* */` comments, and regular expressions are skipped, so an
apostrophe in a comment or a `}` in a regular expression does not end it. A `/` is a regular expression where an
operand may start: at the start, after a punctuator other than `)` and `]`, or after a keyword such as `return` or
`typeof`; elsewhere it divides, as after a postfix `++` or `--` and after a property named as a keyword (`dims.in / 12`).
A tag ends at the first `>` outside brackets, so a comparison in a tag goes in parentheses (`<#if (a > b)>`), and a
`//` comment in a tag is refused, since it would run past the tag's `>`: use `/* */` there. The same reading applies to `ec.resource.expand` and the `${}` in a screen or service
XML attribute: the code of a `${}` is kept as written, as Java's Groovy string kept it, and only the text around it
has its backslashes and backticks escaped.
