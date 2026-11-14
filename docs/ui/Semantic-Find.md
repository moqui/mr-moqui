# Semantic find

Ctrl+F (Cmd+F on a Mac) on a page that turns it on opens a find bar that finds sentences by meaning. The browser's find
matches only the exact words typed. This find also finds a sentence that means what was typed but has other words, or
has the same words in other places. It highlights the exact text of each match and scrolls to it.

## Turn it on in an app

Include the script and attach it to the element that holds the text:

```html
<script defer src="/js/MoquiFind.js"></script>
<script>
  document.addEventListener("DOMContentLoaded", () => moqui.find.attach({ root: "#content" }));
</script>
```

| Option | Default | What it does |
|---|---|---|
| `root` | the body | The element, or a selector, whose text is searched |
| `ignore` | none | A selector of elements whose text is not searched |
| `url` | `/find/rank` | The transition that ranks the sentences |
| `keys` | `Mod+F` | The key that opens the bar. `Mod` is Ctrl, or Cmd on a Mac |
| `limit` | 10 | Matches shown |
| `token` | `moqui.sessionToken` | A function that gives the CSRF token |

`attach` returns `{open(query), close(), search(query), detach()}`.

## Use the bar

| Key | What it does |
|---|---|
| Ctrl+F | Opens the bar. Selected text becomes the query |
| Typing | Searches after a 300 ms pause |
| Enter, Shift+Enter | Next and previous match |
| Esc | Closes the bar |
| Ctrl+F in the bar | Opens the browser's own find |

The list under the input shows the matches, best first. Click one to go to it.

## How it works

1. `MoquiFind.js` cuts the text of `root` into sentences. A sentence ends at `.`, `!` or `?` before a space, at a block
   element or a `br`, and at a line break where line breaks show (a `pre`). Script, style, form fields, hidden elements
   and the bar are left out. Each sentence keeps a DOM Range over its exact text.
2. When the bar opens, the script posts the sentences with an empty query. The server embeds them and keeps the
   vectors in memory, so a search then embeds only the query.
3. A search posts `{query, texts, limit}` to `/find/rank` (`webroot/find.xml`). The transition calls
   `org.moqui.search.SearchServices.rank#Texts`, which calls `ec.search.rankTexts` (`crates/moqui-core/src/search/rank.rs`).
4. A sentence's score is 0.8 × its cosine to the query plus 0.2 × the share of the query's words it has. A sentence
   with all the words typed is ahead of one that only means the same.
5. With no embedding model the transition answers 503. The bar then finds the sentences that have half or more of the
   words of three letters or more, the ones with more of them first, and says that it finds by words.

## Configure the model

The ranking uses the search facade's embedder. A small local model is enough. EmbeddingGemma-300M in llama.cpp embeds
about 100 sentences in 2.5 seconds on a laptop CPU:

```sh
llama-server -m embeddinggemma-300M-Q8_0.gguf --embedding -c 8192 -ub 8192 -b 8192 --port 18291
```

```xml
<llm-facade>
    <endpoint id="embed" url="http://127.0.0.1:18291" local="true"/>
    <model id="embed" endpoint="embed" name="embeddinggemma"/>
    <bind feature="embed" model="embed"/>
</llm-facade>
<search-facade>
    <semantic enabled="true" embedder="llm" dimensions="768"
            query-prefix="task: search result | query: " document-prefix="title: none | text: "/>
</search-facade>
```

`query-prefix` and `document-prefix` are the instructions the model was trained with. Use the ones of your model.
Without them a model ranks worse: in a test page of 107 sentences, the sentence wanted was first with the prefixes and
fifth without them for Qwen3-Embedding-0.6B. See [the LLM embedder](../ai/Gateway.md#the-llm-embedder).

`semantic enabled="true"` on the Postgres backend also tries `CREATE EXTENSION vector`. Without pgvector, stored
semantic search stays off and a warning says so; the find still works, because it stores nothing.

## Test

`moqui test-client webroot --offline` runs `test/client/MoquiFind.spec.js`: the sentence cutting, the ranges, the word
find, and the bar over a stub of `/find/rank`. `cargo test -p moqui-core --lib search::rank` tests the ranking and the
vector cache.
